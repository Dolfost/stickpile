//! Results without waiting and the server's reachability, through the
//! bridge (docs/offline.md, "Reachability and results without waiting";
//! the core's own tests are `stickpile_core`'s `tests/reach.rs`): the
//! two-step search, the long poll on what the library knows of the server,
//! and the check the app asks for. "Slow" is the mock with a search delay,
//! "away" the mock stopped (connections refused).

use std::time::{Instant, SystemTime};

use stickpile_android::{LibraryErrorKind, ServerReach, Target};

use super::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Signed in as alice, synced: the offline cache holds the seeded items.
async fn synced(server: &MockServer, dir: &Path) -> Arc<StickpileLibrary> {
    let lib = signed_in(server, dir);
    lib.sync_now().await.unwrap();
    lib
}

fn searches(server: &MockServer) -> usize {
    requests(server, "GET /api/v1/search")
}

#[tokio::test]
async fn search_two_step_slow_server() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    server.state().search_delay = Some(Duration::from_secs(1));

    let start = Instant::now();
    let answer = lib.search_steps("naahh".into(), Target::Generic, 10).await.unwrap();
    let first_at = start.elapsed();
    assert!(answer.list.offline, "the offline results first");
    assert_eq!(ids(&answer.list.items), vec![items[1].id.clone()]);
    assert!(first_at >= ms(300) && first_at < ms(800), "{first_at:?}");

    let pending = answer.pending.expect("the server's results are to come");
    let online = pending.server_results().await.unwrap().expect("the server answered");
    assert!(!online.offline);
    assert_eq!(ids(&online.items), vec![items[1].id.clone()]);
    assert!(start.elapsed() >= Duration::from_secs(1), "{:?}", start.elapsed());
    assert_eq!(online.items[0].name, "naahh");
    assert_eq!(lib.server_reach(), ServerReach::Reachable);
    assert_eq!(searches(&server), 1, "one request for both steps");
    // Once: the answer was handed over.
    assert_eq!(pending.server_results().await.unwrap(), None);

    // Nobody waiting does not cancel the request: it still answers.
    server.state().requests.clear();
    let answer = lib.search_steps("sleepy".into(), Target::Generic, 10).await.unwrap();
    assert!(answer.list.offline && answer.pending.is_some());
    drop(answer);
    assert!(wait_until(|| searches(&server) == 1).await);

    // A server that answers in time: one step, its results.
    server.state().search_delay = None;
    let answer = lib.search_steps("cat".into(), Target::Discord, 10).await.unwrap();
    assert!(!answer.list.offline && answer.pending.is_none());
    let mut got = ids(&answer.list.items);
    got.sort();
    let mut want = vec![items[0].id.clone(), items[1].id.clone()];
    want.sort();
    assert_eq!(got, want);
}

#[tokio::test]
async fn search_unreachable_answers_offline_at_once() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    server.stop();

    // Refused: the offline results at once, nothing to wait for, and the
    // server is believed away.
    let start = Instant::now();
    let answer = lib.search_steps("fire".into(), Target::Generic, 10).await.unwrap();
    assert!(answer.list.offline && answer.pending.is_none());
    assert_eq!(ids(&answer.list.items), vec![items[3].id.clone()]);
    assert!(start.elapsed() < ms(300), "{:?}", start.elapsed());
    assert!(matches!(lib.server_reach(), ServerReach::Unreachable { .. }));
    assert!(lib.server_unreachable());

    // Believed away: the next search asks nothing, even with the server
    // back, until a check or a request finds it.
    server.restart();
    server.state().requests.clear();
    let start = Instant::now();
    let answer = lib.search_steps("tired".into(), Target::Generic, 10).await.unwrap();
    assert!(start.elapsed() < ms(100), "{:?}", start.elapsed());
    assert!(answer.list.offline && answer.pending.is_none());
    assert_eq!(ids(&answer.list.items), vec![items[2].id.clone()]);
    assert_eq!(searches(&server), 0, "nothing was sent");
}

/// The server's step fails (the request times out after `timeout_secs`,
/// 2 s here): `None`, the offline results stand, and the server is
/// believed away from then on.
#[tokio::test]
async fn pending_search_after_failure_is_none() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    server.state().search_delay = Some(Duration::from_secs(4));

    let answer = lib.search_steps("naahh".into(), Target::Generic, 10).await.unwrap();
    assert!(answer.list.offline);
    assert_eq!(ids(&answer.list.items), vec![items[1].id.clone()]);
    let pending = answer.pending.expect("the server was asked");
    let start = Instant::now();
    assert_eq!(pending.server_results().await.unwrap(), None);
    assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
    assert!(matches!(lib.server_reach(), ServerReach::Unreachable { .. }));
    assert_eq!(pending.server_results().await.unwrap(), None, "still none");

    // The next search does not wait for it.
    let answer = lib.search_steps("cat".into(), Target::Generic, 10).await.unwrap();
    assert!(answer.list.offline && answer.pending.is_none());
}

#[tokio::test]
async fn server_reach_maps_and_changes() {
    let (mut server, _items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = signed_in(&server, dir.path());
    assert_eq!(lib.server_reach(), ServerReach::Unknown);

    // A wait from Unknown ends when a request is answered.
    let waiting = tokio::spawn({
        let lib = lib.clone();
        async move { lib.server_reach_change(ServerReach::Unknown).await }
    });
    tokio::time::sleep(ms(50)).await;
    assert!(!waiting.is_finished(), "nothing changed yet");
    lib.sync_now().await.unwrap();
    let now = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(now.unwrap(), ServerReach::Reachable);
    // Another value than the one seen: at once.
    assert_eq!(
        lib.server_reach_change(ServerReach::Unknown).await.unwrap(),
        ServerReach::Reachable
    );

    // Away: Unreachable, since the first failure.
    server.stop();
    let before = SystemTime::now();
    let waiting = tokio::spawn({
        let lib = lib.clone();
        async move { lib.server_reach_change(ServerReach::Reachable).await }
    });
    lib.search_steps("cat".into(), Target::Generic, 10).await.unwrap();
    let away = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ServerReach::Unreachable { since } = away else {
        panic!("not unreachable: {away:?}");
    };
    assert!(since >= before && since <= SystemTime::now(), "{since:?}");
    assert_eq!(lib.server_reach(), away);
    // The same outage: a wait from it goes on.
    let waiting = tokio::spawn({
        let lib = lib.clone();
        async move { lib.server_reach_change(away).await }
    });
    lib.search_steps("dog".into(), Target::Generic, 10).await.unwrap();
    tokio::time::sleep(ms(50)).await;
    assert!(!waiting.is_finished(), "still the same outage");

    // Back: the check the app asks for finds it.
    server.restart();
    lib.check_server();
    let back = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.unwrap(), ServerReach::Reachable);
    assert!(!lib.server_unreachable());

    // Another server: a wait follows its session (Unknown so far).
    let other = MockServer::start();
    let waiting = tokio::spawn({
        let lib = lib.clone();
        async move { lib.server_reach_change(ServerReach::Reachable).await }
    });
    tokio::time::sleep(ms(50)).await;
    lib.set_server_url(other.url.clone()).await.unwrap();
    let now = tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(now.unwrap(), ServerReach::Unknown);
    assert_eq!(lib.server_reach(), ServerReach::Unknown);
}

/// The network callback calls `check_server` on a thread of the platform's,
/// outside any Tokio runtime.
#[tokio::test]
async fn check_server_from_any_thread() {
    let (mut server, _items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;
    server.stop();
    lib.search_steps("cat".into(), Target::Generic, 10).await.unwrap();
    assert!(lib.server_unreachable());

    server.restart();
    server.state().requests.clear();
    std::thread::spawn({
        let lib = lib.clone();
        move || lib.check_server()
    })
    .join()
    .unwrap();
    assert!(wait_until(|| lib.server_reach() == ServerReach::Reachable).await);
    assert_eq!(requests(&server, "GET /healthz"), 1);

    // Whatever the value: a check while it is reachable goes out too, and
    // keeps it so.
    std::thread::spawn({
        let lib = lib.clone();
        move || lib.check_server()
    })
    .join()
    .unwrap();
    assert!(wait_until(|| requests(&server, "GET /healthz") == 2).await);
    assert_eq!(lib.server_reach(), ServerReach::Reachable);
}

/// In place of the stopped `server`, at its address: a listener that
/// accepts and never answers (a server that hangs). Closed when dropped.
fn hang_in_place_of(server: &mut MockServer) -> std::net::TcpListener {
    server.stop();
    std::net::TcpListener::bind(server.url.trim_start_matches("http://")).unwrap()
}

/// The keyboard asks whether someone is signed in when its Recent tab is
/// empty (`account`): a library opened with the server hanging answers
/// with the cached account after 300 ms, not after the request's timeout
/// (2 s here), and at once once the server is believed away.
#[tokio::test]
async fn keyboard_signed_in_without_waiting() {
    let (mut server, _items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    synced(&server, dir.path()).await;
    let _hung = hang_in_place_of(&mut server);

    let lib = open(dir.path(), &server.url);
    let start = Instant::now();
    let account = lib.account().await.unwrap().expect("the cached account");
    assert_eq!(account.username, "alice");
    assert!(start.elapsed() < ms(800), "{:?}", start.elapsed());

    // The request went on and timed out: believed away, answered at once.
    assert!(wait_until(|| matches!(lib.server_reach(), ServerReach::Unreachable { .. })).await);
    let start = Instant::now();
    assert_eq!(lib.account().await.unwrap().unwrap().username, "alice");
    assert!(start.elapsed() < ms(100), "{:?}", start.elapsed());
}

/// The app's start (`account_steps`): the cached account to go on with,
/// and the server's word on it after.
#[tokio::test]
async fn account_steps_hand_over_the_servers_word() {
    let (mut server, _items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let lib = synced(&server, dir.path()).await;

    // In time: the server's answer, nothing to come.
    let answer = lib.account_steps().await.unwrap();
    assert_eq!(answer.account.unwrap().username, "alice");
    assert!(answer.pending.is_none());

    // Slow: the cached account, then the server confirms it.
    server.state().me_delay = Some(Duration::from_secs(1));
    let answer = lib.account_steps().await.unwrap();
    assert_eq!(answer.account.clone().unwrap().username, "alice");
    let pending = answer.pending.expect("to come");
    let confirmed = pending.server_account().await.unwrap().expect("still signed in");
    assert_eq!(Some(confirmed), answer.account);
    assert_eq!(
        kind(pending.server_account().await),
        LibraryErrorKind::Unreachable,
        "once"
    );

    // Revoked: the cached account, then signed out.
    for t in server.state().tokens.iter_mut() {
        t.info.revoked = true;
    }
    let answer = lib.account_steps().await.unwrap();
    assert_eq!(answer.account.unwrap().username, "alice");
    assert_eq!(answer.pending.expect("to come").server_account().await.unwrap(), None);
    server.state().me_delay = None;
    assert_eq!(lib.account().await.unwrap(), None, "answered in time: signed out");

    // Hanging (a new start): the cached account, and no word from the
    // server: the account stands.
    for t in server.state().tokens.iter_mut() {
        t.info.revoked = false;
    }
    let _hung = hang_in_place_of(&mut server);
    let lib = open(dir.path(), &server.url);
    let answer = lib.account_steps().await.unwrap();
    assert_eq!(answer.account.unwrap().username, "alice");
    let pending = answer.pending.expect("the server was asked");
    assert_eq!(kind(pending.server_account().await), LibraryErrorKind::Unreachable);
}
