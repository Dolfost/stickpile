//! Reachability (docs/offline.md): the watch value and the background
//! checks and their schedule. "Refused" is a closed port (the mock
//! stopped); the schedule runs on paused time with a counted check.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use stickpile_api::TokenRequest;

use super::*;
use crate::client::{ProbeFn, Reach, PROBE_FAST, PROBE_FAST_FOR, PROBE_SLOW, RECENTLY_UNREACHABLE};
use crate::mock::dead_url;
use crate::CoreError;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A checked server for paused time: every check is counted and answered
/// at once, as `up` says.
#[derive(Default)]
struct Checked {
    checks: AtomicUsize,
    up: AtomicBool,
}

impl Checked {
    fn checks(&self) -> usize {
        self.checks.load(Ordering::SeqCst)
    }

    fn set_up(&self, up: bool) {
        self.up.store(up, Ordering::SeqCst);
    }

    /// A fail-fast client whose background check is this one.
    fn client(self: &Arc<Self>) -> ApiClient {
        let me = self.clone();
        let probe: ProbeFn = Arc::new(move || {
            me.checks.fetch_add(1, Ordering::SeqCst);
            let up = me.up.load(Ordering::SeqCst);
            Box::pin(async move {
                if up {
                    Ok(())
                } else {
                    Err(CoreError::Offline("refused".into()))
                }
            })
        });
        // No request leaves this client: reads are refused before a
        // connection while the server is believed away.
        ApiClient::new(&dead_url(), Duration::from_secs(2))
            .unwrap()
            .with_fail_fast_probe(probe)
    }
}

/// Let the tasks that are ready run, without moving paused time.
async fn settle() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn reachability_starts_unknown_and_goes_reachable() {
    let server = MockServer::start();
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token)).with_fail_fast();
    let mut rx = client.reach();
    assert_eq!(*rx.borrow(), Reach::Unknown);
    assert_eq!(client.reach_now(), Reach::Unknown);

    // Any answer counts, an error too (a clone without the token: 401).
    assert!(matches!(client.anonymous().me().await, Err(CoreError::NotLoggedIn)));
    tokio::time::timeout(ms(500), rx.changed()).await.unwrap().unwrap();
    assert_eq!(*rx.borrow_and_update(), Reach::Reachable);
    assert!(!client.believed_unreachable());
    assert_eq!(client.me().await.unwrap().username, "alice");
    assert!(!rx.has_changed().unwrap(), "no news while it stays reachable");
    // Nothing checks a server that answers.
    tokio::time::sleep(ms(200)).await;
    assert_eq!(
        server.state().requests,
        vec!["GET /api/v1/me".to_string(), "GET /api/v1/me".to_string()]
    );

    // Without fail-fast the state is not kept, and never changes.
    let plain = client_for(&server.url, None);
    plain.health().await.unwrap();
    assert_eq!(plain.reach_now(), Reach::Unknown);
    assert!(plain.reach().changed().await.is_err());
    assert_eq!(plain.reach_changed(Reach::Reachable).await, Reach::Unknown);
}

#[tokio::test]
async fn unreachable_after_offline_failure() {
    let mut server = MockServer::start();
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token)).with_fail_fast();
    let rx = client.reach();

    server.stop();
    let before = SystemTime::now();
    assert!(client.me().await.unwrap_err().is_offline());
    let Reach::Unreachable { since } = *rx.borrow() else {
        panic!("{:?}", *rx.borrow());
    };
    assert!(since >= before && since <= SystemTime::now(), "{since:?}");
    assert!(client.believed_unreachable());

    // Reads fail at once without a request; the first check is 15 s off.
    server.restart();
    server.state().requests.clear();
    let start = Instant::now();
    let err = client.sets(None).await.unwrap_err();
    assert!(
        err.is_offline() && err.to_string().contains(RECENTLY_UNREACHABLE),
        "{err}"
    );
    assert!(client.search(&Default::default()).await.unwrap_err().is_offline());
    assert!(start.elapsed() < ms(100), "{:?}", start.elapsed());
    tokio::time::sleep(ms(200)).await;
    assert!(server.state().requests.is_empty(), "{:?}", server.state().requests);

    // A failed write does not move the start of the outage.
    server.stop();
    let login = TokenRequest {
        username: "alice".into(),
        password: "secret".into(),
        totp: None,
        token_name: "t2".into(),
    };
    assert!(client.login(&login).await.unwrap_err().is_offline());
    assert_eq!(*rx.borrow(), Reach::Unreachable { since });
    // A write that is answered ends it.
    server.restart();
    client.login(&login).await.unwrap();
    assert_eq!(*rx.borrow(), Reach::Reachable);
}

#[tokio::test(start_paused = true)]
async fn probe_runs_every_15s_then_60s() {
    let checked = Arc::new(Checked::default());
    let client = checked.client();
    let t0 = tokio::time::Instant::now();
    // The first check finds the server away and starts the outage.
    client.check_now();
    settle().await;
    assert_eq!(checked.checks(), 1);
    assert!(client.believed_unreachable());

    let at = |secs: f64| t0 + Duration::from_secs_f64(secs);
    tokio::time::sleep_until(at(14.9)).await;
    assert_eq!(checked.checks(), 1);
    tokio::time::sleep_until(at(15.1)).await;
    assert_eq!(checked.checks(), 2);
    // Every 15 s for the first 5 minutes: 15, 30, ..., 300.
    assert_eq!(PROBE_FAST_FOR.as_secs() / PROBE_FAST.as_secs(), 20);
    tokio::time::sleep_until(at(300.5)).await;
    assert_eq!(checked.checks(), 1 + 20);
    // Then every 60 s: 360, 420, ...
    tokio::time::sleep_until(at(359.5)).await;
    assert_eq!(checked.checks(), 21);
    tokio::time::sleep_until(at(360.5)).await;
    assert_eq!(checked.checks(), 22);
    tokio::time::sleep_until(at(300.5) + PROBE_SLOW * 5).await;
    assert_eq!(checked.checks(), 26);
    tokio::time::sleep_until(at(3600.5)).await;
    assert_eq!(checked.checks(), 26 + 50);
    assert!(client.believed_unreachable());
}

#[tokio::test(start_paused = true)]
async fn probe_stops_when_reachable() {
    let checked = Arc::new(Checked::default());
    let client = checked.client();
    client.check_now();
    settle().await;
    tokio::time::sleep(Duration::from_secs(31)).await;
    assert_eq!(checked.checks(), 3, "at 0, 15 and 30 s");

    checked.set_up(true);
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert_eq!(checked.checks(), 4);
    assert_eq!(client.reach_now(), Reach::Reachable);
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(checked.checks(), 4, "no checks while it is reachable");

    // The next outage starts them again, on its own schedule.
    checked.set_up(false);
    client.check_now();
    settle().await;
    assert_eq!(checked.checks(), 5);
    assert!(client.believed_unreachable());
    tokio::time::sleep(Duration::from_secs(16)).await;
    assert_eq!(checked.checks(), 6);
}

#[tokio::test(start_paused = true)]
async fn probe_does_not_spin_while_reads_fail() {
    let checked = Arc::new(Checked::default());
    let client = checked.client();
    client.check_now();
    settle().await;
    assert!(client.believed_unreachable());
    // A read every 100 ms for 50 s: all refused at once, and only the
    // schedule checks the server (at 0, 15, 30 and 45 s).
    for _ in 0..500 {
        let err = client.me().await.unwrap_err();
        assert!(err.to_string().contains(RECENTLY_UNREACHABLE), "{err}");
        tokio::time::sleep(ms(100)).await;
    }
    assert_eq!(checked.checks(), 4);
}

/// The check task runs on the runtime of the request that failed; when that
/// runtime ends, the next read starts it again on its own.
#[test]
fn checks_outlive_the_runtime_they_started_on() {
    let paused = || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
    };
    let checked = Arc::new(Checked::default());
    let client = paused().block_on(async {
        let client = checked.client();
        client.check_now();
        settle().await;
        client
    });
    assert_eq!(checked.checks(), 1);
    assert!(client.believed_unreachable());

    paused().block_on(async {
        assert!(client.me().await.unwrap_err().is_offline());
        tokio::time::sleep(PROBE_FAST + ms(500)).await;
        assert_eq!(checked.checks(), 2, "checked again on this runtime");
        checked.set_up(true);
        tokio::time::sleep(PROBE_FAST).await;
        assert_eq!(client.reach_now(), Reach::Reachable);
    });
}

/// The GTK and macOS shells open the session on their runtime, then make
/// it fail-fast and ask for a check from the UI thread, before any request.
#[test]
fn check_server_from_a_ui_thread_before_any_request() {
    let mut server = MockServer::start();
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let session = rt.block_on(Session::open(config_for(&server.url, dir.path()))).unwrap();
    let session = session.with_fail_fast();
    server.stop();
    session.check_server();
    let now = rt.block_on(async {
        tokio::time::timeout(ms(1500), session.reach_changed(Reach::Unknown))
            .await
            .unwrap()
    });
    assert!(now.is_unreachable(), "{now:?}");
}

#[tokio::test]
async fn check_now_probes_at_once() {
    let mut server = MockServer::start();
    let token = server.state().issue_token("alice", "t");
    let client = client_for(&server.url, Some(token)).with_fail_fast();

    server.stop();
    client.me().await.unwrap_err();
    let down = client.reach_now();
    assert!(down.is_unreachable());
    server.restart();
    server.state().requests.clear();
    let start = Instant::now();
    client.check_now();
    let now = tokio::time::timeout(ms(1500), client.reach_changed(down))
        .await
        .unwrap();
    assert_eq!(now, Reach::Reachable);
    assert!(start.elapsed() < ms(1000), "{:?}", start.elapsed());
    assert_eq!(server.state().requests, vec!["GET /healthz".to_string()]);

    // From a thread outside the runtime (a UI thread), with the server
    // there: one check, on the runtime of the last request.
    server.state().requests.clear();
    let other = client.clone();
    std::thread::spawn(move || other.check_now()).join().unwrap();
    for _ in 0..100 {
        if !server.state().requests.is_empty() {
            break;
        }
        tokio::time::sleep(ms(10)).await;
    }
    assert_eq!(server.state().requests, vec!["GET /healthz".to_string()]);
    assert_eq!(client.reach_now(), Reach::Reachable);

    // A server that went away is found without a read.
    server.stop();
    client.check_now();
    let now = tokio::time::timeout(ms(1500), client.reach_changed(Reach::Reachable))
        .await
        .unwrap();
    assert!(now.is_unreachable(), "{now:?}");
}

#[tokio::test(start_paused = true)]
async fn watch_value_flips_when_server_returns() {
    let checked = Arc::new(Checked::default());
    let client = checked.client();
    let mut rx = client.reach();
    client.check_now();
    rx.changed().await.unwrap();
    let down = *rx.borrow_and_update();
    assert!(down.is_unreachable(), "{down:?}");

    // The server comes back: the next scheduled check sees it, and the
    // value flips without any read.
    tokio::time::sleep(Duration::from_secs(20)).await;
    checked.set_up(true);
    let start = tokio::time::Instant::now();
    rx.changed().await.unwrap();
    assert_eq!(*rx.borrow_and_update(), Reach::Reachable);
    let waited = start.elapsed();
    assert!(
        waited >= ms(9900) && waited <= ms(10100),
        "the check at 30 s: {waited:?}"
    );
    assert_eq!(client.reach_changed(down).await, Reach::Reachable);

    // Forgetting an outage makes it unknown again.
    checked.set_up(false);
    client.check_now();
    rx.changed().await.unwrap();
    assert!(rx.borrow_and_update().is_unreachable());
    client.forget_unreachable();
    assert_eq!(*rx.borrow_and_update(), Reach::Unknown);
}
