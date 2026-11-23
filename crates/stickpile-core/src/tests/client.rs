use std::time::Duration;

use axum::http::StatusCode;
use axum::routing::get;
use stickpile_api::*;

use super::*;
use crate::client::{SearchParams, UploadFile};
use crate::mock::{dead_url, MockServer, PAIRING_TTL_SECS};
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
async fn pairing_codes_work_once_and_expire() {
    let server = MockServer::start();
    let anon = client_for(&server.url, None);
    // Creating a code needs a signed-in client.
    assert!(matches!(anon.create_pairing().await, Err(CoreError::NotLoggedIn)));
    let token = server.state().issue_token("alice", "laptop");
    let alice = client_for(&server.url, Some(token));
    let created = alice.create_pairing().await.unwrap();
    assert_eq!(pairing::display(&created.code), created.code);
    let link = pairing::parse_link(&created.pairing_url).unwrap();
    assert_eq!(link.server_url, server.url);
    assert_eq!(link.code, pairing::normalize(&created.code).unwrap());

    let req = |code: &str| PairRequest {
        code: code.into(),
        token_name: "phone".into(),
    };
    let paired = anon.pair(&req(&created.code.to_lowercase())).await.unwrap();
    assert_eq!(paired.user.username, "alice");
    assert_eq!(paired.info.name, "phone");
    let phone = client_for(&server.url, Some(paired.token));
    assert_eq!(phone.me().await.unwrap().username, "alice");

    // Used, wrong and expired codes: the same 401 (an API error, not
    // "not logged in": the route is public).
    let expired = server.state().add_pairing("alice");
    server.state().clock += PAIRING_TTL_SECS;
    for code in [created.code.as_str(), "0000-0000-0000", "nope", expired.as_str()] {
        let err = anon.pair(&req(code)).await.unwrap_err();
        assert_eq!(err.status(), Some(401), "{code}: {err:?}");
        assert_eq!(err.api_code(), Some("unauthorized"));
        assert!(err.to_string().contains("invalid or expired pairing code"), "{err}");
    }
    let fresh = server.state().add_pairing("alice");
    let bad_name = PairRequest {
        code: fresh.clone(),
        token_name: " ".into(),
    };
    assert_eq!(anon.pair(&bad_name).await.unwrap_err().status(), Some(400));
    anon.pair(&req(&fresh)).await.unwrap();
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
async fn upload_path_streams_a_file() {
    let server = MockServer::start();
    let token = server.state().issue_token("alice", "t");
    let set = server.state().add_set("alice", "a", "A", Visibility::Private);
    let client = client_for(&server.url, Some(token));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("upload-1.tmp");
    let gif = gif_bytes(4, 7);
    std::fs::write(&path, &gif).unwrap();

    let meta = UploadMeta {
        keywords: Some(vec!["meow".into()]),
        ..Default::default()
    };
    let res = client
        .upload_path(&set.id, &path, "cat.gif", Some(&meta))
        .await
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].file, "cat.gif");
    let item = res[0].item.as_ref().unwrap();
    assert_eq!(item.name, "cat", "named after the part's file name");
    assert_eq!(item.keywords, vec!["meow"]);
    // The server received the file as it is on disk.
    let stored = server
        .state()
        .items
        .iter()
        .find(|i| i.item.id == item.id)
        .map(|i| i.files[&Variant::Original].clone());
    assert_eq!(stored, Some(gif));

    // No metadata; and a file that is not there.
    let res = client.upload_path(&set.id, &path, "again.gif", None).await.unwrap();
    assert!(res[0].already_in_set);
    let missing = dir.path().join("gone.png");
    let e = client
        .upload_path(&set.id, &missing, "gone.png", None)
        .await
        .unwrap_err();
    assert!(
        matches!(&e, CoreError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
        "{e}"
    );
    assert!(e.to_string().contains("gone.png"), "{e}");
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

#[tokio::test]
async fn fail_fast_answers_reads_at_once_while_the_server_is_away() {
    let mut server = MockServer::start();
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token)).with_fail_fast();
    let plain = client_for(&server.url, None);
    assert_eq!(client.me().await.unwrap().username, "alice");
    assert!(!client.believed_unreachable());

    server.stop();
    assert!(client.me().await.unwrap_err().is_offline());
    assert!(client.believed_unreachable());
    server.restart();
    server.state().requests.clear();
    let err = client.me().await.unwrap_err();
    assert!(err.is_offline(), "{err:?}");
    assert!(err.to_string().contains(crate::client::RECENTLY_UNREACHABLE), "{err}");
    assert!(
        client.anonymous().health().await.unwrap_err().is_offline(),
        "the clones know it too"
    );
    plain.health().await.expect("a client without fail-fast tries");
    assert_eq!(server.state().requests, vec!["GET /healthz".to_string()]);

    // A write goes out, and the server's answer ends it.
    let login = TokenRequest {
        username: "alice".into(),
        password: "secret".into(),
        totp: None,
        token_name: "t2".into(),
    };
    client.login(&login).await.unwrap();
    assert!(!client.believed_unreachable());
    assert_eq!(client.me().await.unwrap().username, "alice");

    // Refreshing by hand forgets it too.
    server.stop();
    client.me().await.unwrap_err();
    server.restart();
    client.forget_unreachable();
    assert_eq!(client.me().await.unwrap().username, "alice");
}

#[tokio::test]
async fn export_formats_publish_and_whatsapp() {
    use crate::client::SetArchive;
    let (server, items) = seeded_server();
    let set_id = items[0].set_id.clone();
    let private_id = items[3].set_id.clone();
    server.state().add_user("bob", "pw", Role::User);
    let alice = client_for(&server.url, Some(server.state().issue_token("alice", "t")));
    let bob = client_for(&server.url, Some(server.state().issue_token("bob", "t")));
    let dir = tempfile::tempdir().unwrap();

    // Every archive format downloads to a file; readers may, others not.
    for format in [SetArchive::Zip, SetArchive::Tar, SetArchive::TarGz] {
        let dest = dir.path().join(format!("cats.{}", format.extension()));
        let n = bob.download_export(&set_id, format, &dest).await.unwrap();
        assert!(n > 0);
        assert_eq!(std::fs::read(&dest).unwrap().len() as u64, n);
        let err = bob
            .download_export(&private_id, format, &dir.path().join("x"))
            .await
            .unwrap_err();
        assert_eq!(err.status(), Some(404));
    }
    assert!(std::fs::read(dir.path().join("cats.zip")).unwrap().starts_with(b"PK"));

    // Publishing: the owner only; one job per platform.
    let list = alice.publications(&set_id).await.unwrap();
    assert_eq!(list[0].platform, "telegram");
    assert!(list[0].configured);
    assert_eq!(bob.publications(&set_id).await.unwrap_err().status(), Some(403));
    let req = PublishRequest {
        platforms: vec!["telegram".into(), "telegram".into()],
        options: [("telegram".to_string(), serde_json::json!({"title": "Cats"}))].into(),
    };
    let jobs = alice.publish(&set_id, &req).await.unwrap();
    assert_eq!(jobs.jobs.len(), 1);
    assert_eq!(jobs.jobs[0].kind, JobKind::TelegramExport);
    assert_eq!(bob.publish(&set_id, &req).await.unwrap_err().status(), Some(403));
    let bad = PublishRequest {
        platforms: vec!["myspace".into()],
        ..PublishRequest::default()
    };
    let err = alice.publish(&set_id, &bad).await.unwrap_err();
    assert_eq!(err.status(), Some(400));
    assert_eq!(err.api_code(), Some("bad_request"));

    // WhatsApp: build, status, the zip and one pack.
    assert!(bob.whatsapp_status(&set_id).await.unwrap().build.is_none());
    assert_eq!(
        bob.download_whatsapp(&set_id, &dir.path().join("none.zip"))
            .await
            .unwrap_err()
            .status(),
        Some(404)
    );
    let job = bob.whatsapp_build(&set_id).await.unwrap();
    assert_eq!(job.kind, JobKind::WhatsappExport);
    let job = bob
        .wait_job(&job.id, Duration::from_millis(10), Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(job.status, JobStatus::Done);
    let status = bob.whatsapp_status(&set_id).await.unwrap();
    assert!(status.current);
    let build = status.build.unwrap();
    assert_eq!(build.packs[0].stickers.len(), 3);
    let zip = dir.path().join("wa/cats.zip");
    assert!(bob.download_whatsapp(&set_id, &zip).await.unwrap() > 0);
    let pack = dir.path().join("wa/cats.wastickers");
    assert!(bob.download_whatsapp_pack(&set_id, 1, &pack).await.unwrap() > 0);
    assert_eq!(
        bob.download_whatsapp_pack(&set_id, 2, &pack)
            .await
            .unwrap_err()
            .status(),
        Some(404)
    );
}

/// A 0.7.0 server has no rechecks: every recheck call says the server is
/// too old. A newer server's 404 for an unknown recheck stays a 404.
#[tokio::test]
async fn rechecks_on_a_server_without_them() {
    let not_found = || async {
        (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"code": "not_found", "message": "not found"})),
        )
    };
    // As 0.7.0 answers any /api path it does not know.
    let old = axum::Router::new().route("/api/{*rest}", axum::routing::any(not_found));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, old).await.unwrap() });
    let client = client_for(&url, Some("spk_x".into()));
    let too_old = |e: CoreError| matches!(&e, CoreError::Other(m) if m == crate::client::SERVER_TOO_OLD_FOR_RECHECKS);
    assert!(too_old(
        client.start_recheck(&StartRecheck::default()).await.unwrap_err()
    ));
    assert!(too_old(
        client.admin_start_recheck(&StartRecheck::default()).await.unwrap_err()
    ));
    assert!(too_old(client.rechecks().await.unwrap_err()));
    assert!(too_old(client.recheck("x").await.unwrap_err()));
    assert!(too_old(client.recheck_failures("x").await.unwrap_err()));
    assert!(too_old(client.cancel_recheck("x").await.unwrap_err()));

    // A server with rechecks: an unknown one is just not found.
    let new = axum::Router::new()
        .route(routes::RECHECKS, get(|| async { axum::Json(Vec::<Recheck>::new()) }))
        .route("/api/{*rest}", axum::routing::any(not_found));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, new).await.unwrap() });
    let client = client_for(&url, Some("spk_x".into()));
    assert!(client.rechecks().await.unwrap().is_empty());
    let err = client.recheck("x").await.unwrap_err();
    assert_eq!(err.status(), Some(404));
    assert_eq!(err.api_code(), Some("not_found"));
}

/// A listener whose backlog is full: a connection to it gets no answer
/// (Linux drops the SYN), like a host that is gone. The streams hold the
/// backlog.
#[cfg(target_os = "linux")]
fn blackhole() -> (tokio::net::TcpListener, Vec<std::net::TcpStream>, String) {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = socket.listen(1).unwrap();
    let addr = listener.local_addr().unwrap();
    let mut held = Vec::new();
    while let Ok(stream) = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)) {
        held.push(stream);
        assert!(held.len() < 16, "the backlog does not fill");
    }
    (listener, held, format!("http://{addr}"))
}

/// Reads connect within 2 s, so a host that is gone is recognised quickly;
/// a transfer connects within `timeout_secs` (here 4 s) as before.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn interactive_connect_timeout_is_2s() {
    use std::time::Instant;
    let ms = Duration::from_millis;
    let (_listener, _held, url) = blackhole();
    let client = ApiClient::new(&url, Duration::from_secs(4)).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("file");
    let read = async {
        let start = Instant::now();
        let err = client.me().await.unwrap_err();
        (start.elapsed(), err)
    };
    let transfer = async {
        let start = Instant::now();
        let err = client
            .download("/api/v1/items/x/file/original", &dest)
            .await
            .unwrap_err();
        (start.elapsed(), err)
    };
    let ((read_took, read_err), (transfer_took, transfer_err)) = tokio::join!(read, transfer);
    assert!(read_err.is_offline(), "{read_err}");
    assert!(transfer_err.is_offline(), "{transfer_err}");
    assert!(
        read_took >= ms(1900) && read_took < ms(3000),
        "a read connects within 2 s: {read_took:?}"
    );
    assert!(
        transfer_took >= ms(3900),
        "a transfer connects within timeout_secs: {transfer_took:?}"
    );
}
