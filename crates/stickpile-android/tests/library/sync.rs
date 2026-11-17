//! `sync_if_due`, `sync_now`, `sync_state`.

use stickpile_android::LibraryErrorKind;
use stickpile_core::mock::dead_url;

use super::*;

const ME: &str = "GET /api/v1/me";

#[tokio::test]
async fn sync_if_due_follows_the_policy() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let before = lib.sync_state();
    assert!(before.due, "never synced: due");
    assert_eq!(
        (before.last_sync, before.last_error, before.pending_usage),
        (None, None, 0)
    );

    let state = lib.sync_if_due().await.unwrap();
    assert!(!state.due);
    assert!(state.last_sync.is_some());
    assert_eq!(state.last_error, None);
    assert_eq!(requests(&server, ME), 1);
    // The cache is filled: offline search finds the items.
    let cached = stickpile_core::cache::Cache::open(&dir.path().join("cache"), 64).unwrap();
    assert_eq!(cached.item_count().unwrap(), items.len());

    // Not due again for ten minutes.
    let state = lib.sync_if_due().await.unwrap();
    assert!(!state.due);
    assert_eq!(requests(&server, ME), 1, "no second sync");
    assert_eq!(lib.sync_state(), state);
    // sync_now does not wait for that.
    lib.sync_now().await.unwrap();
    assert_eq!(requests(&server, ME), 2);
}

#[tokio::test]
async fn failed_syncs_are_retried_later() {
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&stickpile_core::mock::MockServer::start(), dir.path());
    // Point it at nothing.
    lib.set_server_url(dead_url()).await.unwrap();
    assert!(lib.sync_state().due);
    assert_eq!(kind(lib.sync_if_due().await), LibraryErrorKind::Unreachable);
    let state = lib.sync_state();
    assert_eq!(state.last_error, Some(LibraryErrorKind::Unreachable));
    assert_eq!(state.last_sync, None);
    assert!(!state.due, "retried after 60 s, not now");
    // Not due: no attempt, no error.
    assert_eq!(lib.sync_if_due().await.unwrap(), state);
    // sync_now tries anyway.
    assert_eq!(kind(lib.sync_now().await), LibraryErrorKind::Unreachable);
}

#[tokio::test]
async fn a_good_sync_clears_the_error() {
    let (mut server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    server.stop();
    assert_eq!(kind(lib.sync_now().await), LibraryErrorKind::Unreachable);
    assert_eq!(lib.sync_state().last_error, Some(LibraryErrorKind::Unreachable));
    server.restart();
    let state = lib.sync_now().await.unwrap();
    assert_eq!(state.last_error, None);
    assert!(state.last_sync.is_some());
}

#[tokio::test]
async fn signed_out_syncs_fail() {
    let (server, _) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = open(dir.path(), &server.url);
    assert_eq!(kind(lib.sync_if_due().await), LibraryErrorKind::NotSignedIn);
    assert_eq!(lib.sync_state().last_error, Some(LibraryErrorKind::NotSignedIn));
    assert_eq!(kind(lib.sync_now().await), LibraryErrorKind::NotSignedIn);
}

#[tokio::test]
async fn a_reopened_library_remembers_a_recent_sync() {
    let (server, _) = seeded_server();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    // The mock's clock is in 2023: that sync is long ago.
    lib.sync_now().await.unwrap();
    drop(lib);
    let lib = open(dir.path(), &server.url);
    let state = lib.sync_state();
    assert!(state.due);
    assert_eq!(state.last_sync, Some(server.state().clock), "the server's time of it");

    // A sync a minute ago (server time): not due after a restart.
    server.state().clock = now - 60;
    lib.sync_now().await.unwrap();
    drop(lib);
    let lib = open(dir.path(), &server.url);
    let state = lib.sync_state();
    assert!(!state.due);
    assert_eq!(state.last_sync, Some(now - 60));
    lib.sync_if_due().await.unwrap();
    assert_eq!(requests(&server, ME), 2, "no sync on reopening");
}

/// The offers come from the cache: a sync says when it changed it, so
/// that offers on screen (for `v:` typed before the first sync was over)
/// are asked for again.
#[tokio::test]
async fn a_sync_says_when_it_changed_the_offers() {
    let (server, items) = seeded_server();
    {
        let mut st = server.state();
        let it = st.items.iter_mut().find(|i| i.item.id == items[0].id).unwrap();
        it.item.vibe = Some("grumpy".into());
    }
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    let offered =
        |lib: &StickpileLibrary| -> Vec<String> { lib.hints("v:".into()).into_iter().map(|h| h.label).collect() };
    assert!(!offered(&lib).contains(&"grumpy".to_string()), "nothing cached yet");
    assert!(!lib.sync_state().cache_changed);

    let state = lib.sync_if_due().await.unwrap();
    assert!(state.cache_changed, "the first sync fills the cache");
    assert!(offered(&lib).contains(&"grumpy".to_string()), "{:?}", offered(&lib));

    assert!(!lib.sync_if_due().await.unwrap().cache_changed, "none was due");
    assert!(
        !lib.sync_now().await.unwrap().cache_changed,
        "nothing new on the server"
    );
    server.state().rename_item(&items[1].id, "naahh bro");
    assert!(lib.sync_now().await.unwrap().cache_changed);
}
