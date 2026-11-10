use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::get;
use stickpile_api::*;

use super::*;
use crate::client::{SearchParams, UploadFile};
use crate::mock::{dead_url, MockServer};
use crate::CoreError;

#[tokio::test]
async fn login_me_and_token_revocation() {
    let server = MockServer::start();
    let anon = client_for(&server.url, None);
    let req = TokenRequest {
        username: "alice".into(),
        password: "secret".into(),
        totp: None,
        token_name: "t".into(),
    };
    let created = anon.login(&req).await.unwrap();
    assert!(created.token.starts_with("spk_"));
    let client = anon.clone().with_token(Some(created.token.clone()));
    assert_eq!(client.me().await.unwrap().username, "alice");

    let id = client.current_token_id().await.unwrap();
    assert_eq!(id.as_deref(), Some(created.info.id.as_str()));
    client.revoke_token(&created.info.id).await.unwrap();
    let err = client.me().await.unwrap_err();
    assert!(err.is_unauthorized(), "{err}");
    assert_eq!(err.api_code(), Some("unauthorized"));
}

#[tokio::test]
async fn api_errors_are_decoded() {
    let server = MockServer::start();
    let anon = client_for(&server.url, None);
    let bad = TokenRequest {
        username: "alice".into(),
        password: "nope".into(),
        totp: None,
        token_name: "t".into(),
    };
    match anon.login(&bad).await.unwrap_err() {
        CoreError::Api { status, error } => {
            assert_eq!(status, 401);
            assert_eq!(error.code, "unauthorized");
            assert!(error.message.contains("invalid"));
        }
        other => panic!("{other:?}"),
    }
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token));
    let err = client.set("doesnotexist").await.unwrap_err();
    assert_eq!(err.status(), Some(404));
    assert_eq!(err.api_code(), Some("not_found"));
    assert_eq!(err.to_string(), "404: not found (not_found)");
}

#[tokio::test]
async fn totp_is_required_when_enabled() {
    let server = MockServer::start();
    server.state().users[0].totp = Some("123456".into());
    let anon = client_for(&server.url, None);
    let mut req = TokenRequest {
        username: "alice".into(),
        password: "secret".into(),
        totp: None,
        token_name: "t".into(),
    };
    let err = anon.login(&req).await.unwrap_err();
    assert!(err.is_totp_required(), "{err}");
    req.totp = Some("000000".into());
    let err = anon.login(&req).await.unwrap_err();
    assert!(!err.is_totp_required() && err.is_unauthorized(), "{err}");
    req.totp = Some("123456".into());
    anon.login(&req).await.unwrap();
}

#[tokio::test]
async fn missing_token_is_not_logged_in() {
    let server = MockServer::start();
    let client = client_for(&server.url, None);
    assert!(matches!(client.me().await, Err(CoreError::NotLoggedIn)));
    assert!(matches!(client.sets(None).await, Err(CoreError::NotLoggedIn)));
    client.health().await.unwrap();
    // A wrong password on the public login route stays an API error.
    let bad = TokenRequest {
        username: "alice".into(),
        password: "x".into(),
        totp: None,
        token_name: "t".into(),
    };
    assert_eq!(client.login(&bad).await.unwrap_err().status(), Some(401));
}

#[tokio::test]
async fn tailnet_identity_works_without_a_token() {
    let server = MockServer::start();
    server.state().tailnet_user = Some("alice".into());
    let client = client_for(&server.url, None);
    assert_eq!(client.me().await.unwrap().username, "alice");
    let created = client
        .create_token(&CreateToken {
            name: "laptop".into(),
            expires_at: None,
        })
        .await
        .unwrap();
    let with_token = client_for(&server.url, Some(created.token));
    assert_eq!(with_token.current_token_id().await.unwrap(), Some(created.info.id));
}

#[tokio::test]
async fn connection_refused_is_offline() {
    let client = client_for(&dead_url(), Some("spk_x".into()));
    let err = client.me().await.unwrap_err();
    assert!(err.is_offline(), "{err:?}");
    assert!(client.health().await.unwrap_err().is_offline());
}

#[tokio::test]
async fn timeout_is_offline() {
    let server = MockServer::start();
    server.state().search_delay = Some(Duration::from_secs(5));
    let token = server.state().issue_token("alice", "t");
    let client = ApiClient::new(&server.url, Duration::from_millis(300))
        .unwrap()
        .with_token(Some(token));
    let started = std::time::Instant::now();
    let err = client
        .search(&SearchParams {
            q: "cat".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err.is_offline(), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn non_api_error_bodies() {
    let app = axum::Router::new()
        .route("/api/v1/me", get(|| async { (StatusCode::BAD_GATEWAY, "Bad Gateway") }))
        .route(
            "/api/v1/sets",
            get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
        )
        .route("/api/v1/tokens", get(|| async { "this is not json" }))
        .route("/api/v1/profiles", get(|| async { (StatusCode::NOT_FOUND, "") }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = client_for(&url, Some("spk_x".into()));

    assert!(
        client.me().await.unwrap_err().is_offline(),
        "502 without ApiError body = proxy without backend"
    );
    match client.sets(None).await.unwrap_err() {
        CoreError::Api { status: 500, error } => {
            assert_eq!(error.code, "internal");
            assert_eq!(error.message, "boom");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(client.tokens().await.unwrap_err(), CoreError::Other(m) if m.contains("unexpected response")));
    let err = client.profiles().await.unwrap_err();
    assert_eq!(err.api_code(), Some("not_found"));
}

#[tokio::test]
async fn upload_reports_duplicates() {
    let server = MockServer::start();
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token));
    let a = client
        .create_set(&CreateSet {
            slug: "a".into(),
            name: "A".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let b = client
        .create_set(&CreateSet {
            slug: "b".into(),
            name: "B".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let conflict = client
        .create_set(&CreateSet {
            slug: "a".into(),
            name: "A".into(),
            ..Default::default()
        })
        .await;
    assert_eq!(conflict.unwrap_err().status(), Some(409));

    let png = png_bytes(8, 8, 9);
    let file = UploadFile {
        filename: "cat.png".into(),
        bytes: png.clone(),
        mime: None,
    };
    let meta = [UploadMeta {
        name: Some("Cat".into()),
        keywords: Some(vec!["meow".into()]),
        ..Default::default()
    }];
    let res = client.upload(&a.id, vec![file.clone()], Some(&meta)).await.unwrap();
    assert_eq!(res.len(), 1);
    let item = res[0].item.as_ref().unwrap();
    assert_eq!(item.name, "Cat");
    assert_eq!(item.keywords, vec!["meow"]);

    let again = client.upload(&a.id, vec![file.clone()], None).await.unwrap();
    assert!(again[0].already_in_set);
    let other = client.upload(&b.id, vec![file], None).await.unwrap();
    assert_eq!(other[0].near_duplicates.len(), 1);
    assert_eq!(other[0].near_duplicates[0].existing.set_slug, "a");

    let bad = UploadFile {
        filename: "notes.txt".into(),
        bytes: b"hello".to_vec(),
        mime: None,
    };
    let res = client.upload(&a.id, vec![bad], None).await.unwrap();
    assert!(res[0].error.is_some());
}

#[tokio::test]
async fn import_dir_and_export_round_trip() {
    let server = MockServer::start();
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token));
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("my-cats");
    std::fs::create_dir_all(&src).unwrap();
    let png = png_bytes(10, 10, 1);
    let gif = gif_bytes(2, 5);
    std::fs::write(src.join("001-grumpy.png"), &png).unwrap();
    std::fs::write(src.join("002-naahh.gif"), &gif).unwrap();
    std::fs::write(src.join("notes.txt"), "not media").unwrap();
    std::fs::write(
        src.join("set.toml"),
        r#"name = "My cats"
visibility = "public"

[items."001-grumpy.png"]
name = "grumpy"
keywords = ["cat", "angry"]

[items."002-naahh.gif"]
keywords = ["nope"]
emojis = ["🙅"]
"#,
    )
    .unwrap();

    let (report, _warnings) = client.import_dir(&src, None).await.unwrap();
    assert_eq!(report.created, 2, "{report:?}");
    let set = report.set.unwrap();
    assert_eq!(set.name, "My cats");
    assert_eq!(set.visibility, Visibility::Public);

    // Re-import into the same set: nothing new.
    let (again, _) = client.import_dir(&src, Some(&set.id)).await.unwrap();
    assert_eq!(again.created, 0);
    assert_eq!(again.unchanged, 2);

    let out = dir.path().join("export");
    let bundle = client.export_to_dir(&set.id, &out).await.unwrap();
    assert_eq!(bundle.files.len(), 2);
    let mut names: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["001-grumpy.png", "002-naahh.gif", "set.toml"]);
    assert_eq!(std::fs::read(out.join("001-grumpy.png")).unwrap(), png);
    assert_eq!(std::fs::read(out.join("002-naahh.gif")).unwrap(), gif);
    let manifest = stickpile_setfmt::Manifest::parse(&std::fs::read_to_string(out.join("set.toml")).unwrap()).unwrap();
    assert_eq!(manifest.name, "My cats");
    assert_eq!(manifest.items["001-grumpy.png"].keywords, vec!["cat", "angry"]);
    assert_eq!(manifest.items["002-naahh.gif"].emojis, vec!["🙅"]);
}

#[tokio::test]
async fn files_likes_jobs_and_gitpack() {
    let (server, items) = seeded_server();
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token));
    let (bytes, ct) = client.item_file(&items[0].id, Variant::Original).await.unwrap();
    assert_eq!(bytes, png_bytes(20, 20, 1));
    assert_eq!(ct.as_deref(), Some("image/png"));
    assert_eq!(
        client
            .item_file(&items[0].id, Variant::TgSticker)
            .await
            .unwrap_err()
            .status(),
        Some(404)
    );

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("sub/x.gif");
    let n = client
        .download_item_file(&items[1].id, Variant::Original, &dest)
        .await
        .unwrap();
    assert_eq!(n as usize, std::fs::read(&dest).unwrap().len());
    assert_eq!(
        std::fs::read_dir(dest.parent().unwrap()).unwrap().count(),
        1,
        "no temp files left"
    );

    assert_eq!(
        client.like(&items[0].id).await.unwrap(),
        LikeState { liked: true, likes: 1 }
    );
    assert_eq!(
        client.unlike(&items[0].id).await.unwrap(),
        LikeState { liked: false, likes: 0 }
    );

    let created = client
        .create_gitpack(&CreateGitPack {
            url: "https://example.com/packs/frogs.git".into(),
            branch: None,
            subdir: None,
            mode: GitMode::ReadOnly,
            slug: Some("frogs".into()),
        })
        .await
        .unwrap();
    assert_eq!(created.set().slug, "frogs");
    let crate::client::GitPackResponse::Created(c) = created else {
        panic!("expected GitPackCreated")
    };
    let job = client
        .wait_job(&c.job.id, Duration::from_millis(10), Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(job.status, JobStatus::Done);
    let pulled = client.pull_set(&c.set.id).await.unwrap();
    assert_eq!(pulled.kind, JobKind::GitPull);
}

#[tokio::test]
async fn admin_routes_require_admin() {
    let server = MockServer::start();
    server.state().add_user("root", "pw", Role::Admin);
    let user = client_for(&server.url, Some(server.state().issue_token("alice", "t")));
    assert_eq!(user.admin_users().await.unwrap_err().status(), Some(403));
    let admin = client_for(&server.url, Some(server.state().issue_token("root", "t")));
    assert_eq!(admin.admin_users().await.unwrap().len(), 2);
    admin.admin_storage().await.unwrap();
    admin.admin_jobs(Some(JobStatus::Failed)).await.unwrap();
    assert_eq!(admin.admin_audit(None, Some(10)).await.unwrap()[0].action, "login");
    let inv = admin
        .admin_create_invitation(&CreateInvitation {
            role: Role::User,
            expires_in: Some(60),
        })
        .await
        .unwrap();
    assert!(inv.url.contains(&inv.invitation.id));
}
