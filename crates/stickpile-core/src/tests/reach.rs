//! Reachability and results without waiting (docs/offline.md): the watch
//! value, the background checks and their schedule, and searches that
//! never wait for a server that does not answer (the short connect timeout
//! of reads is tested in `client.rs`). "Hung" is a listener that accepts and
//! never answers (like a tailnet peer that is gone), "refused" a closed
//! port, "slow" the mock with a delay; the schedule runs on paused time
//! with a counted check.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use stickpile_api::{SearchHit, TargetApp, TokenRequest};

use super::*;
use crate::client::{ProbeFn, Reach, PROBE_FAST, PROBE_FAST_FOR, PROBE_SLOW, RECENTLY_UNREACHABLE};
use crate::mock::dead_url;
use crate::session::OFFLINE_AFTER;
use crate::CoreError;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// A server that accepts connections (the kernel does, into the backlog)
/// and never answers. Closed when the listener is dropped.
fn hung_server() -> (std::net::TcpListener, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
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

fn ids(hits: &[SearchHit]) -> Vec<String> {
    let mut ids: Vec<String> = hits.iter().map(|h| h.item.id.clone()).collect();
    ids.sort();
    ids
}

fn sorted(items: &[&ItemSummary]) -> Vec<String> {
    let mut ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    ids.sort();
    ids
}

/// What the offline index answers for `cat`: the two cats among them.
fn offline_cats(session: &Session, target: Option<TargetApp>, items: &[ItemSummary]) -> Vec<String> {
    let found = ids(&session.search_offline("cat", target, 10).unwrap().hits);
    assert!(
        found.contains(&items[0].id) && found.contains(&items[1].id),
        "{found:?}"
    );
    found
}

/// A session on a server that never answers, its cache holding `items`.
async fn hung_session(url: &str, dir: &Path, items: &[ItemSummary], fail_fast: bool) -> Session {
    let session = Session::open(config_for(url, dir)).await.unwrap();
    session.cache().upsert_items(items).unwrap();
    if fail_fast {
        session.with_fail_fast()
    } else {
        session
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

#[tokio::test]
async fn search_unreachable_answers_offline_at_once() {
    let (_server, items) = seeded_server();
    let (_hung, url) = hung_server();
    let dir = tempfile::tempdir().unwrap();
    let session = hung_session(&url, dir.path(), &items, true).await;

    // A read finds the server away (it times out after `timeout_secs`).
    assert!(session.client().me().await.unwrap_err().is_offline());
    assert!(session.reach().borrow().is_unreachable());

    let start = Instant::now();
    let steps = session.search_steps("cat", Some(TargetApp::Generic), 10).await.unwrap();
    let took = start.elapsed();
    assert!(steps.first.offline && steps.then.is_none());
    assert_eq!(
        ids(&steps.first.hits),
        offline_cats(&session, Some(TargetApp::Generic), &items)
    );
    // Well under the 2 s connect timeout: no request was made.
    assert!(took < ms(100), "{took:?}");
}

#[tokio::test]
async fn search_slow_server_gives_offline_then_server() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await.with_fail_fast();
    session.sync().await.unwrap();
    server.state().search_delay = Some(Duration::from_secs(1));

    let start = Instant::now();
    let steps = session.search_steps("naahh", None, 10).await.unwrap();
    let first_at = start.elapsed();
    assert!(steps.first.offline);
    assert_eq!(ids(&steps.first.hits), sorted(&[&items[1]]));
    assert!(
        first_at >= OFFLINE_AFTER && first_at < OFFLINE_AFTER + ms(300),
        "{first_at:?}"
    );

    let server_step = steps.then.expect("the server's answer is to come");
    let online = server_step.wait().await.unwrap().expect("the server answered");
    let second_at = start.elapsed();
    assert!(!online.offline);
    assert_eq!(ids(&online.hits), sorted(&[&items[1]]));
    assert!(second_at >= Duration::from_secs(1), "{second_at:?}");
    assert_eq!(*session.reach().borrow(), Reach::Reachable);
    let searches = server
        .state()
        .requests
        .iter()
        .filter(|r| r.starts_with("GET /api/v1/search"))
        .count();
    assert_eq!(searches, 1, "one request for both steps");
}

#[tokio::test]
async fn search_refused_server_gives_offline_at_once() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await.with_fail_fast();
    session.sync().await.unwrap();
    server.stop();

    let start = Instant::now();
    let steps = session.search_steps("naahh", None, 10).await.unwrap();
    assert!(steps.first.offline && steps.then.is_none());
    assert_eq!(ids(&steps.first.hits), sorted(&[&items[1]]));
    assert!(start.elapsed() < OFFLINE_AFTER, "{:?}", start.elapsed());
    assert!(session.reach().borrow().is_unreachable());
}

#[tokio::test]
async fn search_fast_server_gives_server_only() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await.with_fail_fast();
    session.sync().await.unwrap();

    let start = Instant::now();
    let steps = session.search_steps("cat", Some(TargetApp::Discord), 10).await.unwrap();
    let took = start.elapsed();
    assert!(!steps.first.offline && steps.then.is_none());
    assert_eq!(ids(&steps.first.hits), sorted(&[&items[0], &items[1]]));
    assert!(took < OFFLINE_AFTER, "{took:?}");
    assert_eq!(*session.reach().borrow(), Reach::Reachable);
}

#[tokio::test]
async fn search_hung_server_marks_unreachable_and_keeps_offline() {
    let (_server, items) = seeded_server();
    let (_hung, url) = hung_server();
    let dir = tempfile::tempdir().unwrap();
    let session = hung_session(&url, dir.path(), &items, true).await;
    assert_eq!(*session.reach().borrow(), Reach::Unknown);

    let start = Instant::now();
    let steps = session.search_steps("cat", None, 10).await.unwrap();
    let first_at = start.elapsed();
    assert!(steps.first.offline);
    assert_eq!(ids(&steps.first.hits), offline_cats(&session, None, &items));
    assert!(
        first_at >= OFFLINE_AFTER && first_at < OFFLINE_AFTER + ms(300),
        "{first_at:?}"
    );
    // The request times out (`timeout_secs` = 2): nothing replaces the
    // offline results, and the server is believed away.
    let then = steps.then.expect("the server was asked");
    assert!(then.wait().await.unwrap().is_none());
    let failed_at = start.elapsed();
    assert!(failed_at >= Duration::from_secs(2), "{failed_at:?}");
    assert!(session.reach().borrow().is_unreachable());

    // The next search does not ask it.
    let start = Instant::now();
    let steps = session.search_steps("cat", None, 10).await.unwrap();
    assert!(steps.first.offline && steps.then.is_none());
    assert!(start.elapsed() < ms(100), "{:?}", start.elapsed());
}

/// The CLI's search: no fail-fast, the server asked every time and waited
/// for, the offline cache when it fails.
#[tokio::test]
async fn cli_search_unchanged() {
    let (mut server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await;
    session.sync().await.unwrap();

    let online = session.search("cat", Some(TargetApp::Discord), 10).await.unwrap();
    assert!(!online.offline);
    assert_eq!(ids(&online.hits), sorted(&[&items[0], &items[1]]));

    // Slow: the server's results, however long it takes within the timeout.
    server.state().search_delay = Some(Duration::from_secs(1));
    let start = Instant::now();
    let slow = session.search("naahh", None, 10).await.unwrap();
    assert!(!slow.offline);
    assert!(start.elapsed() >= Duration::from_secs(1), "{:?}", start.elapsed());

    // Refused: the offline cache at once, and the next search tries again.
    server.state().search_delay = None;
    server.stop();
    let start = Instant::now();
    let refused = session.search("naahh", None, 10).await.unwrap();
    assert!(refused.offline);
    assert_eq!(ids(&refused.hits), sorted(&[&items[1]]));
    assert!(start.elapsed() < ms(500), "{:?}", start.elapsed());
    server.restart();
    assert!(!session.search("naahh", None, 10).await.unwrap().offline);
    assert_eq!(*session.reach().borrow(), Reach::Unknown, "no state is kept");
    tokio::time::sleep(ms(200)).await;
    assert!(
        !server.state().requests.iter().any(|r| r == "GET /healthz"),
        "nothing checks the server"
    );

    // Hung: the timeout (`timeout_secs` = 2), then the offline cache, on
    // every search.
    let (_hung, url) = hung_server();
    let other = tempfile::tempdir().unwrap();
    let session = hung_session(&url, other.path(), &items, false).await;
    for _ in 0..2 {
        let start = Instant::now();
        let out = session.search("cat", None, 10).await.unwrap();
        assert!(out.offline);
        assert_eq!(ids(&out.hits), offline_cats(&session, None, &items));
        assert!(start.elapsed() >= Duration::from_secs(2), "{:?}", start.elapsed());
    }
}

/// Alice as the server knows her, for the cache of a session on another
/// server (one that never answers, or is gone).
async fn alice(server: &MockServer) -> stickpile_api::User {
    let token = server.state().issue_token("alice", "for the cache");
    client_for(&server.url, Some(token)).me().await.unwrap()
}

/// An app opened with the server away: the account the cache last saw,
/// after OFFLINE_AFTER at most (not the request's timeout), and at once
/// once the server is believed unreachable; with nothing cached the server
/// is still asked and waited for (a fresh install).
#[tokio::test]
async fn cold_start_with_unreachable_server_uses_cached_account() {
    let (server, items) = seeded_server();
    let me = alice(&server).await;
    let (_hung, url) = hung_server();
    let dir = tempfile::tempdir().unwrap();
    let session = hung_session(&url, dir.path(), &items, true).await;
    session.cache().set_me(&me).unwrap();
    assert_eq!(*session.reach().borrow(), Reach::Unknown);

    // Unknown: the server is asked, and does not answer in time.
    let start = Instant::now();
    let steps = session.account_steps().await.unwrap();
    let first_at = start.elapsed();
    assert_eq!(steps.first.username, "alice");
    assert!(steps.cached);
    assert!(
        first_at >= OFFLINE_AFTER && first_at < OFFLINE_AFTER + ms(300),
        "{first_at:?}"
    );
    // Its answer never comes: the request times out (`timeout_secs` = 2)
    // and the cached account stands; the server is believed away.
    let then = steps.then.expect("the server was asked");
    assert!(then.wait().await.unwrap().is_none());
    assert!(session.reach().borrow().is_unreachable());

    // Believed away: the cached account at once, nothing asked.
    let start = Instant::now();
    let steps = session.account_steps().await.unwrap();
    assert!(steps.cached && steps.then.is_none());
    assert_eq!(steps.first.username, "alice");
    assert!(start.elapsed() < ms(100), "{:?}", start.elapsed());

    // A server that is gone (refused): the cached account at once.
    let gone = tempfile::tempdir().unwrap();
    let session = Session::open(config_for(&dead_url(), gone.path()))
        .await
        .unwrap()
        .with_fail_fast();
    session.cache().set_me(&me).unwrap();
    let start = Instant::now();
    let steps = session.account_steps().await.unwrap();
    assert!(steps.cached && steps.then.is_none());
    assert!(start.elapsed() < OFFLINE_AFTER, "{:?}", start.elapsed());

    // Nothing cached (a fresh install): the server decides, waited for.
    let fresh = tempfile::tempdir().unwrap();
    let session = hung_session(&url, fresh.path(), &[], true).await;
    let start = Instant::now();
    let err = session.account_steps().await.unwrap_err();
    assert!(err.is_offline(), "{err}");
    assert!(start.elapsed() >= Duration::from_secs(2), "{:?}", start.elapsed());
}

/// A revoked token signs out once the server answers, also when the cached
/// account was handed over first; a server that answers in time says so at
/// once, and confirms a token that works.
#[tokio::test]
async fn revoked_token_still_signs_out_when_server_answers() {
    let (server, _items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let session = session_for(&server, dir.path()).await.with_fail_fast();
    session.sync().await.unwrap();

    // In time: the server's account.
    let steps = session.account_steps().await.unwrap();
    assert!(!steps.cached && steps.then.is_none());
    assert_eq!(steps.first.username, "alice");

    // Slow: the cached account first, the server's word after it.
    server.state().me_delay = Some(Duration::from_secs(1));
    let steps = session.account_steps().await.unwrap();
    assert!(steps.cached);
    let confirmed = steps.then.expect("to come").wait().await.unwrap();
    assert_eq!(confirmed.map(|u| u.username).as_deref(), Some("alice"));

    // Revoked meanwhile: the cached account first, then 401.
    for t in server.state().tokens.iter_mut() {
        t.info.revoked = true;
    }
    let start = Instant::now();
    let steps = session.account_steps().await.unwrap();
    assert!(steps.cached);
    assert_eq!(steps.first.username, "alice");
    assert!(start.elapsed() < OFFLINE_AFTER + ms(300), "{:?}", start.elapsed());
    let err = steps.then.expect("to come").wait().await.unwrap_err();
    assert!(err.is_unauthorized(), "{err}");
    assert!(start.elapsed() >= Duration::from_secs(1), "{:?}", start.elapsed());
    assert_eq!(*session.reach().borrow(), Reach::Reachable, "a 401 is an answer");

    // Answered in time: the 401 at once.
    server.state().me_delay = None;
    let err = session.account_steps().await.unwrap_err();
    assert!(err.is_unauthorized(), "{err}");
}
