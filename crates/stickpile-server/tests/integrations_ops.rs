//! Operational pieces: `healthcheck` and `check-config` subcommands, the
//! trusted socket's permissions, token revocation on admin password resets,
//! rate limiting of callers with an unknown IP, and `PATCH /api/v1/me`.

mod common;

use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use common::TestServer;
use reqwest::Method;
use serde_json::json;
use stickpile_api::{routes, AuditEntry, User};

fn server_bin(args: &[&str], config: Option<&Path>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stickpile-server"));
    if let Some(c) = config {
        cmd.arg("--config").arg(c);
    }
    cmd.args(args).env_remove("STICKPILE_CONFIG").env("RUST_LOG", "off");
    cmd.output().unwrap()
}

async fn bin(args: Vec<String>, config: Option<std::path::PathBuf>) -> Output {
    tokio::task::spawn_blocking(move || {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        server_bin(&a, config.as_deref())
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthcheck_subcommand() {
    let srv = TestServer::start().await;
    let ok = bin(
        vec!["healthcheck".into(), "--url".into(), srv.url(routes::HEALTH)],
        None,
    )
    .await;
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    assert!(String::from_utf8_lossy(&ok.stdout).contains("healthy"));

    // From the configuration (server.listen), no --url.
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    std::fs::write(&cfg, toml::to_string(&srv.state.config).unwrap()).unwrap();
    let ok = bin(vec!["healthcheck".into()], Some(cfg.clone())).await;
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));

    // A wildcard listen address is checked on loopback.
    let mut wild = srv.state.config.clone();
    wild.server.listen = format!("0.0.0.0:{}", srv.state.config.server.listen.port())
        .parse()
        .unwrap();
    std::fs::write(&cfg, toml::to_string(&wild).unwrap()).unwrap();
    let ok = bin(vec!["healthcheck".into()], Some(cfg)).await;
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));

    // Not 2xx.
    let bad = bin(
        vec!["healthcheck".into(), "--url".into(), srv.url("/api/v1/nope")],
        None,
    )
    .await;
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("HTTP 404"));

    // Nothing listening.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let down = bin(
        vec![
            "healthcheck".into(),
            "--url".into(),
            format!("http://127.0.0.1:{port}/healthz"),
        ],
        None,
    )
    .await;
    assert_eq!(down.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&down.stderr).contains("unhealthy"));

    // Accepts but never answers: gives up after ~3 s.
    let hang = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/healthz", hang.local_addr().unwrap());
    let started = Instant::now();
    let slow = bin(vec!["healthcheck".into(), "--url".into(), url], None).await;
    assert_eq!(slow.status.code(), Some(1));
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(2500) && took < Duration::from_secs(30),
        "{took:?}"
    );
    drop(hang);

    // Only plain http.
    let tls = bin(
        vec!["healthcheck".into(), "--url".into(), "https://localhost/healthz".into()],
        None,
    )
    .await;
    assert_eq!(tls.status.code(), Some(1));
}

#[tokio::test]
async fn check_config_warns_about_missing_models_and_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let data = dir.path().join("data");
    std::fs::write(&cfg, format!("[server]\ndata_dir = {:?}\n", data.display().to_string())).unwrap();
    let out = bin(vec!["check-config".into()], Some(cfg.clone())).await;
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("configuration OK"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("warning"));

    let models = dir.path().join("models");
    std::fs::create_dir_all(models.join("clip-vit-b32")).unwrap();
    std::fs::write(models.join(stickpile_ai::layout::VISION), b"onnx").unwrap();
    std::fs::write(
        &cfg,
        format!(
            "[server]\ndata_dir = {:?}\n[ai]\nenabled = true\nmodel_dir = {:?}\n\
             [telegram]\nenabled = true\nbot_token_file = {:?}\n",
            data.display().to_string(),
            models.display().to_string(),
            dir.path().join("missing-token").display().to_string()
        ),
    )
    .unwrap();
    let out = bin(vec!["check-config".into()], Some(cfg)).await;
    assert!(out.status.success(), "warnings do not fail the check");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("warning: ai.enabled = true but model files are missing"),
        "{err}"
    );
    assert!(
        err.contains(stickpile_ai::layout::CLIP_TEXT) && err.contains(stickpile_ai::layout::CLIP_TOKENIZER),
        "{err}"
    );
    assert!(
        !err.contains(stickpile_ai::layout::VISION),
        "present files are not listed: {err}"
    );
    assert!(err.contains("telegram.bot_token_file"), "{err}");
}

#[tokio::test]
async fn trusted_socket_is_never_wider_than_its_mode() {
    let srv = TestServer::start().await;
    let meta = std::fs::metadata(&srv.socket).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o660, "the default socket_mode");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ts.sock");
    let l = stickpile_server::bind_unix(&path, "0600").unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["ts.sock"], "no staging leftovers");
    // Connections reach the renamed socket.
    let accept = tokio::spawn(async move { l.accept().await.map(|_| ()) });
    tokio::net::UnixStream::connect(&path).await.unwrap();
    accept.await.unwrap().unwrap();
    // A stale socket is replaced; anything else is refused.
    stickpile_server::bind_unix(&path, "0o640").unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
    let file = dir.path().join("file");
    std::fs::write(&file, "x").unwrap();
    assert!(stickpile_server::bind_unix(&file, "0600").is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
}

#[tokio::test]
async fn admin_password_reset_revokes_tokens_self_service_does_not() {
    let srv = TestServer::start().await;
    let admin = srv.admin("root").await;
    let alice = srv.user("alice").await;
    srv.put(
        &alice,
        routes::ME_PASSWORD,
        json!({"current_password": alice.password, "new_password": "a-new-password"}),
    )
    .await
    .expect(204);
    srv.get(&alice, routes::ME).await.expect(200);

    srv.patch(
        &admin,
        &routes::path(routes::ADMIN_USER, &[&alice.id]),
        json!({"password": "reset-by-admin-1"}),
    )
    .await
    .expect(200);
    let r = srv.get(&alice, routes::ME).await.expect(401);
    assert_eq!(r.code(), "unauthorized");
    let audit: Vec<AuditEntry> = srv.get(&admin, routes::ADMIN_AUDIT).await.expect(200).json();
    let entry = audit.iter().find(|a| a.action == "user.update").unwrap();
    assert_eq!(entry.details.as_ref().unwrap()["tokens_revoked"], 1);
    // The admin's own tokens are untouched; the new password works.
    srv.get(&admin, routes::ME).await.expect(200);
    srv.send(
        None,
        Method::POST,
        routes::AUTH_TOKEN,
        Some(json!({"username": "alice", "password": "reset-by-admin-1", "token_name": "t"})),
    )
    .await
    .expect(200);
}

#[tokio::test]
async fn unknown_client_ips_share_a_login_bucket() {
    let srv = TestServer::with_config(|c| c.limits.login_per_minute = 2).await;
    let allowed = (2 * stickpile_server::security::UNKNOWN_IP_LOGIN_FACTOR) as usize;
    let mut statuses = Vec::new();
    // The shared bucket refills one attempt every 6 s here; allow a slow
    // machine a few extra before the limit must hit.
    for i in 0..allowed + 5 {
        // The trusted socket without X-Forwarded-For: no client IP.
        let body = json!({"username": format!("nobody{i}"), "password": "wrong", "token_name": "t"}).to_string();
        let r = srv.unix("POST", routes::AUTH_TOKEN, &[], Some(&body)).await;
        statuses.push(r.status.as_u16());
        if r.status == 429 {
            break;
        }
    }
    let first_limited = statuses.iter().position(|s| *s == 429).expect("never rate limited");
    assert!(first_limited >= allowed, "{statuses:?}");
    assert!(statuses[..first_limited].iter().all(|s| *s == 401), "{statuses:?}");
    // A caller with a known IP (TCP) is not affected.
    let r = srv
        .send(
            None,
            Method::POST,
            routes::AUTH_TOKEN,
            Some(json!({"username": "x", "password": "y", "token_name": "t"})),
        )
        .await;
    assert_eq!(r.status, 401);
}

#[tokio::test]
async fn update_me_sets_display_name_and_telegram_id() {
    let srv = TestServer::start().await;
    let alice = srv.user("alice").await;
    let me: User = srv.get(&alice, routes::ME).await.expect(200).json();
    assert_eq!(me.telegram_user_id, None);
    let me: User = srv
        .patch(
            &alice,
            routes::ME,
            json!({"display_name": "Alice A.", "telegram_user_id": 12345}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!(
        (me.display_name.as_str(), me.telegram_user_id),
        ("Alice A.", Some(12345))
    );
    // Absent fields stay; null clears; empty display name resets it.
    let me: User = srv.patch(&alice, routes::ME, json!({})).await.expect(200).json();
    assert_eq!(me.telegram_user_id, Some(12345));
    let me: User = srv
        .patch(
            &alice,
            routes::ME,
            json!({"telegram_user_id": null, "display_name": ""}),
        )
        .await
        .expect(200)
        .json();
    assert_eq!((me.display_name.as_str(), me.telegram_user_id), ("alice", None));
    srv.patch(&alice, routes::ME, json!({"telegram_user_id": -5}))
        .await
        .expect(400);
    srv.patch(&alice, routes::ME, json!({"display_name": "x\u{7}"}))
        .await
        .expect(400);
    let r = srv.send(None, Method::PATCH, routes::ME, Some(json!({}))).await;
    assert_eq!(r.status, 401);
}
