//! Opening the library, the server URL and the account.

use std::os::unix::fs::PermissionsExt;

use stickpile_android::{Account, AuthKind, LibraryDirs, LibraryErrorKind, StickpileLibrary};
use stickpile_core::mock::{dead_url, MockServer};

use super::*;

#[tokio::test]
async fn opens_with_the_defaults_or_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    // No config file: the defaults, nothing written.
    let lib = StickpileLibrary::new(dirs(dir.path())).unwrap();
    assert_eq!(lib.server_url(), "http://localhost:8080");
    assert!(!config_path(dir.path()).exists());
    assert!(dir.path().join("cache/cache.sqlite3").exists(), "the cache is opened");
    drop(lib);

    let lib = open(dir.path(), "https://stickers.example");
    assert_eq!(lib.server_url(), "https://stickers.example");
    assert!(lib.recent(10).is_empty());
    assert_eq!(lib.pending_usage(), 0);
}

#[tokio::test]
async fn refuses_relative_paths_and_broken_config_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut d = dirs(dir.path());
    d.cache_dir = "cache".into();
    assert_eq!(invalid_field(StickpileLibrary::new(d)), "cache_dir");
    let d = LibraryDirs {
        config_file: "config.toml".into(),
        ..dirs(dir.path())
    };
    assert_eq!(invalid_field(StickpileLibrary::new(d)), "config_file");

    std::fs::write(config_path(dir.path()), "server_url = [").unwrap();
    assert_eq!(invalid_field(StickpileLibrary::new(dirs(dir.path()))), "config_file");
    std::fs::write(config_path(dir.path()), "server_url = \"ftp://x\"").unwrap();
    assert_eq!(invalid_field(StickpileLibrary::new(dirs(dir.path()))), "config_file");
}

#[tokio::test]
async fn set_server_url_saves_it_and_reopens() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = open(dir.path(), &dead_url());
    assert_eq!(lib.account().await.unwrap(), None, "a fresh install is signed out");

    lib.set_server_url(format!(" {}/ ", server.url)).await.unwrap();
    assert_eq!(lib.server_url(), server.url);
    // Saved, keeping the rest of the file.
    let text = std::fs::read_to_string(config_path(dir.path())).unwrap();
    assert!(text.contains(&format!("server_url = \"{}\"", server.url)), "{text}");
    assert!(
        text.contains("# written by the test") && text.contains("timeout_secs = 2"),
        "{text}"
    );
    // Reopened for it: signed out there, and a login goes to it.
    assert_eq!(lib.account().await.unwrap(), None);
    let account = lib.login_password("alice".into(), "secret".into(), None).await.unwrap();
    assert_eq!(account.server_url, server.url);

    // A new library reads it back.
    drop(lib);
    let again = StickpileLibrary::new(dirs(dir.path())).unwrap();
    assert_eq!(again.server_url(), server.url);

    // Bad URLs change nothing.
    for bad in [
        "",
        "stickers.example",
        "ftp://stickers.example",
        "https://",
        "https://x/?a=b",
    ] {
        assert_eq!(
            invalid_field(again.set_server_url(bad.into()).await),
            "server_url",
            "{bad:?}"
        );
    }
    assert_eq!(again.server_url(), server.url);
    assert_eq!(std::fs::read_to_string(config_path(dir.path())).unwrap(), text);
}

/// Make `server` take `secret` as a token of its alice, so that a request
/// carrying it would be signed in there.
fn accept_token(server: &MockServer, secret: &str) {
    let mut st = server.state();
    st.issue_token("alice", "copied");
    st.tokens.last_mut().unwrap().secret = secret.to_string();
}

/// Requests `server` received whose `METHOD /path` starts with `prefix`.
fn requests_starting(server: &MockServer, prefix: &str) -> usize {
    server.state().requests.iter().filter(|r| r.starts_with(prefix)).count()
}

#[tokio::test]
async fn a_new_server_url_signs_out_first() {
    let (old, items) = seeded_server();
    let new = MockServer::start();
    let dir = tempfile::tempdir().unwrap();
    let secret = store_token(&old, dir.path());
    // Were the old token sent to the new server, alice would be signed in there.
    accept_token(&new, &secret);
    let lib = open(dir.path(), &old.url);
    lib.sync_now().await.unwrap();
    lib.record_use(items[0].id.clone(), "original".into(), "".into())
        .unwrap();
    assert_eq!(lib.account().await.unwrap().unwrap().username, "alice");

    lib.set_server_url(new.url.clone()).await.unwrap();
    assert_eq!(lib.server_url(), new.url);
    assert!(!token_path(dir.path()).exists(), "the token file is gone");
    assert_eq!(lib.account().await.unwrap(), None, "signed out");
    // The old server got the unsent use and revoked the token.
    assert_eq!(requests_starting(&old, "DELETE /api/v1/tokens/"), 1);
    assert!(old.state().tokens.iter().all(|t| t.info.revoked));
    assert_eq!(old.state().usage.len(), 1);
    // The cache no longer holds the old server's data.
    assert!(lib.recent(10).is_empty());
    assert_eq!(lib.sync_state().last_sync, None);
    // A new library is signed out too.
    let again = StickpileLibrary::new(dirs(dir.path())).unwrap();
    assert_eq!(again.server_url(), new.url);
    assert_eq!(again.account().await.unwrap(), None);
}

#[tokio::test]
async fn leaving_an_unreachable_server_still_deletes_the_token() {
    let (mut old, _) = seeded_server();
    let new = MockServer::start();
    let dir = tempfile::tempdir().unwrap();
    let secret = store_token(&old, dir.path());
    accept_token(&new, &secret);
    let lib = open(dir.path(), &old.url);
    old.stop();

    lib.set_server_url(new.url.clone()).await.unwrap();
    assert_eq!(lib.server_url(), new.url);
    assert!(!token_path(dir.path()).exists());
    assert_eq!(lib.account().await.unwrap(), None);
    assert!(
        old.state().tokens.iter().all(|t| !t.info.revoked),
        "not revoked: the old server was down"
    );
}

#[tokio::test]
async fn a_hanging_old_server_does_not_hold_the_change_up() {
    // Connections to it are made (the kernel's backlog) but never answered.
    let hanging = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let old_url = format!("http://{}", hanging.local_addr().unwrap());
    let new = MockServer::start();
    let dir = tempfile::tempdir().unwrap();
    stickpile_core::token::write_token(&token_path(dir.path()), "spk_old").unwrap();
    let lib = open(dir.path(), &old_url);

    let started = std::time::Instant::now();
    lib.set_server_url(new.url.clone()).await.unwrap();
    // At most the config's timeout (2 s here) for the old server.
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    assert!(!token_path(dir.path()).exists());
    assert_eq!(lib.account().await.unwrap(), None);
    drop(hanging);
}

#[tokio::test]
async fn the_same_server_url_keeps_the_session() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    lib.set_server_url(format!(" {}/", server.url)).await.unwrap();
    assert!(token_path(dir.path()).exists());
    assert_eq!(lib.account().await.unwrap().unwrap().username, "alice");
    assert_eq!(requests_starting(&server, "DELETE "), 0);
    assert!(server.state().tokens.iter().all(|t| !t.info.revoked));
    assert!(lib.sync_state().last_sync.is_some(), "the cache is kept");
}

#[tokio::test]
async fn pairing_with_another_server_replaces_the_token() {
    let (old, _) = seeded_server();
    let new = MockServer::start();
    let dir = tempfile::tempdir().unwrap();
    let secret = store_token(&old, dir.path());
    let lib = open(dir.path(), &old.url);
    let code = new.state().add_pairing("alice");

    let account = lib.login_pairing(new.url.clone(), code).await.unwrap();
    assert_eq!(
        (account.server_url.as_str(), account.auth),
        (new.url.as_str(), AuthKind::Token)
    );
    // The code went out alone, before any request that could carry a token.
    assert_eq!(new.state().requests, vec!["POST /api/v1/auth/pair".to_string()]);
    // The new token replaced the old one, which the old server revoked.
    let stored = stickpile_core::token::read_token(&token_path(dir.path()))
        .unwrap()
        .unwrap();
    assert_ne!(stored, secret);
    assert!(new
        .state()
        .tokens
        .iter()
        .any(|t| t.secret == stored && t.info.name == "Android (phone)"));
    assert!(old.state().tokens.iter().all(|t| t.info.revoked));
    assert_eq!(lib.account().await.unwrap().unwrap().auth, AuthKind::Token);
    assert_eq!(lib.server_url(), new.url);
}

#[tokio::test]
async fn account_signed_in_signed_out_and_offline() {
    let (mut server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = open(dir.path(), &server.url);
    assert_eq!(lib.account().await.unwrap(), None, "no token: signed out");

    store_token(&server, dir.path());
    let lib = open(dir.path(), &server.url);
    let alice = Account {
        username: "alice".into(),
        display_name: None,
        server_url: server.url.clone(),
        auth: AuthKind::Token,
        is_admin: false,
    };
    assert_eq!(lib.account().await.unwrap(), Some(alice.clone()));

    // Offline: the account the cache last saw (a sync stores it).
    lib.sync_now().await.unwrap();
    server.stop();
    assert_eq!(lib.account().await.unwrap(), Some(alice));
    // Nothing cached and no token: signed out, even offline.
    let other = tempfile::tempdir().unwrap();
    let lib = open(other.path(), &server.url);
    assert_eq!(lib.account().await.unwrap(), None);
    // A token but nothing cached: whether it still works is unknown.
    let third = tempfile::tempdir().unwrap();
    stickpile_core::token::write_token(&token_path(third.path()), "made-up-token").unwrap();
    let lib = open(third.path(), &server.url);
    assert_eq!(kind(lib.account().await), LibraryErrorKind::Unreachable);

    // A revoked token: signed out.
    server.restart();
    for t in server.state().tokens.iter_mut() {
        t.info.revoked = true;
    }
    let lib = open(dir.path(), &server.url);
    assert_eq!(lib.account().await.unwrap(), None);
}

#[tokio::test]
async fn admins_and_display_names() {
    let server = MockServer::start();
    {
        let mut st = server.state();
        st.add_user("root", "pw", stickpile_api::Role::Admin);
        st.users.last_mut().unwrap().user.display_name = "The Root".into();
    }
    let dir = tempfile::tempdir().unwrap();
    let token = server.state().issue_token("root", "test");
    stickpile_core::token::write_token(&token_path(dir.path()), &token).unwrap();
    let lib = open(dir.path(), &server.url);
    let account = lib.account().await.unwrap().unwrap();
    assert!(account.is_admin);
    assert_eq!(account.display_name.as_deref(), Some("The Root"));
}

#[tokio::test]
async fn tailnet_identity_and_login() {
    let server = MockServer::start();
    let dir = tempfile::tempdir().unwrap();
    let lib = open(dir.path(), &server.url);
    // Not on the tailnet.
    assert_eq!(lib.login_tailnet().await.unwrap(), None);
    assert!(!token_path(dir.path()).exists());

    server.state().tailnet_user = Some("alice".into());
    let account = lib.account().await.unwrap().unwrap();
    assert_eq!((account.username.as_str(), account.auth), ("alice", AuthKind::Tailnet));

    let account = lib.login_tailnet().await.unwrap().unwrap();
    assert_eq!((account.username.as_str(), account.auth), ("alice", AuthKind::Token));
    assert!(token_path(dir.path()).exists());
    assert!(server.state().tokens.iter().any(|t| t.info.name == "Android (phone)"));
    assert_eq!(lib.account().await.unwrap().unwrap().auth, AuthKind::Token);

    // Offline it fails.
    let dead = tempfile::tempdir().unwrap();
    let lib = open(dead.path(), &dead_url());
    assert_eq!(kind(lib.login_tailnet().await), LibraryErrorKind::Unreachable);
}

#[tokio::test]
async fn login_password_stores_the_token() {
    let server = MockServer::start();
    let dir = tempfile::tempdir().unwrap();
    let lib = open(dir.path(), &server.url);

    let wrong = lib.login_password("alice".into(), "nope".into(), None).await;
    assert_eq!(kind(wrong), LibraryErrorKind::Unauthorized);
    assert!(!token_path(dir.path()).exists());

    let account = lib
        .login_password(" alice ".into(), "secret".into(), None)
        .await
        .unwrap();
    assert_eq!((account.username.as_str(), account.auth), ("alice", AuthKind::Token));
    assert_eq!(account.server_url, server.url);
    let mode = std::fs::metadata(token_path(dir.path())).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(server.state().tokens.iter().any(|t| t.info.name == "Android (phone)"));
    assert_eq!(lib.account().await.unwrap().unwrap().username, "alice");
    // A new library finds the token.
    let again = StickpileLibrary::new(dirs(dir.path())).unwrap();
    assert_eq!(again.account().await.unwrap().unwrap().auth, AuthKind::Token);
}

#[tokio::test]
async fn login_password_with_two_factor() {
    let server = MockServer::start();
    server.state().users[0].totp = Some("123456".into());
    let dir = tempfile::tempdir().unwrap();
    let lib = open(dir.path(), &server.url);
    let no_code = lib.login_password("alice".into(), "secret".into(), None).await;
    assert_eq!(invalid_field(no_code), "totp");
    let blank = lib
        .login_password("alice".into(), "secret".into(), Some(" ".into()))
        .await;
    assert_eq!(invalid_field(blank), "totp");
    let wrong = lib
        .login_password("alice".into(), "secret".into(), Some("000000".into()))
        .await;
    assert_eq!(kind(wrong), LibraryErrorKind::Unauthorized);
    lib.login_password("alice".into(), "secret".into(), Some(" 123456 ".into()))
        .await
        .unwrap();

    let dead = tempfile::tempdir().unwrap();
    let lib = open(dead.path(), &dead_url());
    let offline = lib.login_password("alice".into(), "secret".into(), None).await;
    assert_eq!(kind(offline), LibraryErrorKind::Unreachable);
}

#[tokio::test]
async fn login_pairing_sets_the_server_and_stores_the_token() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    // Configured for another server, as before scanning a pairing link.
    let first = dead_url();
    let lib = open(dir.path(), &first);
    let config_before = std::fs::read_to_string(config_path(dir.path())).unwrap();

    // Input that cannot be right: nothing is sent, nothing changes.
    let code = server.state().add_pairing("alice");
    assert_eq!(
        invalid_field(lib.login_pairing(server.url.clone(), "12-34".into()).await),
        "code"
    );
    assert_eq!(
        invalid_field(lib.login_pairing("nope".into(), code.clone()).await),
        "server_url"
    );
    // A wrong code: unauthorized, nothing changes.
    let wrong = lib.login_pairing(server.url.clone(), "0000-0000-0000".into()).await;
    assert_eq!(kind(wrong), LibraryErrorKind::Unauthorized);
    assert_eq!(lib.server_url(), first);
    assert!(!token_path(dir.path()).exists());
    assert_eq!(std::fs::read_to_string(config_path(dir.path())).unwrap(), config_before);
    // An unreachable server.
    let offline = lib.login_pairing(dead_url(), code.clone()).await;
    assert_eq!(kind(offline), LibraryErrorKind::Unreachable);

    // The scanned link's values, the code as typed on a phone.
    let typed = code.to_lowercase().replace('-', " ");
    let account = lib.login_pairing(server.url.clone(), typed).await.unwrap();
    assert_eq!(account.username, "alice");
    assert_eq!(
        (account.auth, account.server_url.as_str()),
        (AuthKind::Token, server.url.as_str())
    );
    assert_eq!(lib.server_url(), server.url);
    let text = std::fs::read_to_string(config_path(dir.path())).unwrap();
    assert!(text.contains(&format!("server_url = \"{}\"", server.url)), "{text}");
    {
        let st = server.state();
        assert!(st.pairings.iter().all(|p| p.used));
        assert!(st.tokens.iter().any(|t| t.info.name == "Android (phone)"));
    }
    // Signed in from now on, also in a new library.
    assert_eq!(lib.account().await.unwrap().unwrap().username, "alice");
    let found = lib
        .search("grumpy".into(), stickpile_android::Target::Generic, 5)
        .await
        .unwrap();
    assert_eq!(ids(&found.items), vec![items[0].id.clone()]);
    let again = StickpileLibrary::new(dirs(dir.path())).unwrap();
    assert_eq!(again.account().await.unwrap().unwrap().auth, AuthKind::Token);

    // A used code.
    let used = lib.login_pairing(server.url.clone(), code).await;
    assert_eq!(kind(used), LibraryErrorKind::Unauthorized);
}

#[tokio::test]
async fn logout_deletes_and_revokes_the_token() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    lib.sync_now().await.unwrap();
    lib.logout().await.unwrap();
    assert!(!token_path(dir.path()).exists());
    assert!(server.state().tokens.iter().all(|t| t.info.revoked));
    assert_eq!(lib.account().await.unwrap(), None);
    assert_eq!(lib.sync_state().last_sync, None, "the account's data is gone");
    // Signed out already: nothing to do.
    lib.logout().await.unwrap();

    // Offline: deleted here all the same.
    let mut server = server;
    let lib = signed_in(&server, dir.path());
    server.stop();
    lib.logout().await.unwrap();
    assert!(!token_path(dir.path()).exists());
}
