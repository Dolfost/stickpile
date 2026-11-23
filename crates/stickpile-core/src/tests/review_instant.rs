//! Review of the instant-search round (docs/offline.md, "Reachability"):
//! the background check of a server believed unreachable, where the
//! schedule tests in `reach.rs` do not go.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::client::{ProbeFn, PROBE_SLOW};
use crate::mock::dead_url;
use crate::{ApiClient, CoreError};

/// Let the tasks that are ready run, without moving paused time.
async fn settle() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

/// A fail-fast client whose check always finds the server away, after
/// `takes` (paused time), counting the checks.
fn counted(checks: &Arc<AtomicUsize>, takes: Duration) -> ApiClient {
    let checks = checks.clone();
    let probe: ProbeFn = Arc::new(move || {
        checks.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(takes).await;
            Err(CoreError::Offline("no route to host".into()))
        })
    });
    ApiClient::new(&dead_url(), Duration::from_secs(2))
        .unwrap()
        .with_fail_fast_probe(probe)
}

/// A session the shells drop (the GTK popup and the macOS app open a new
/// one on every opening while a login is needed, and when the settings
/// change; Android when the server or the account changes) stops checking
/// its server: nothing reads what the checks find any more. Here the
/// check task holds the shared reachability and goes on checking the old
/// server every 60 s for as long as the process lives, one more task for
/// every session opened during the outage.
#[tokio::test(start_paused = true)]
async fn a_dropped_client_stops_checking_its_server() {
    let checks = Arc::new(AtomicUsize::new(0));
    let client = counted(&checks, Duration::ZERO);
    client.check_now();
    settle().await;
    assert!(client.believed_unreachable());
    assert_eq!(checks.load(Ordering::SeqCst), 1);

    drop(client);
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(
        checks.load(Ordering::SeqCst),
        1,
        "the dropped client's server was checked again"
    );
}

/// A check asked for while one is out ([`ApiClient::check_now`]: Android
/// asks when a network appears, the popup when it opens) is dropped. When
/// the check that is out fails (it went out before the network was there),
/// nothing checks the server until the schedule's next turn: up to
/// [`PROBE_SLOW`] (60 s) later once the outage is five minutes old, with
/// the server reachable over the network that just appeared.
#[tokio::test(start_paused = true)]
async fn a_check_asked_for_while_one_is_out_is_not_lost() {
    let checks = Arc::new(AtomicUsize::new(0));
    // A check to a host that is gone takes the connect timeout (2 s).
    let client = counted(&checks, Duration::from_secs(2));
    client.check_now();
    settle().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(client.believed_unreachable());
    assert_eq!(checks.load(Ordering::SeqCst), 1);

    // Into the slow phase: a check every 60 s. Wait until one is out.
    tokio::time::sleep(Duration::from_secs(5 * 60)).await;
    let before = checks.load(Ordering::SeqCst);
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if checks.load(Ordering::SeqCst) > before {
            break;
        }
    }
    // The network appears while that check waits for its connection.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let asked = checks.load(Ordering::SeqCst);
    client.check_now();
    // The check that was out fails at 2 s; the one asked for should follow.
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        checks.load(Ordering::SeqCst) > asked,
        "the check asked for while one was out never went out: the next is the schedule's, {}s after the last",
        PROBE_SLOW.as_secs()
    );
}

/// "Reads ... connect within 2 s ...; `timeout_secs` still bounds the whole
/// request, so a server that accepts the connection and never answers takes
/// that long" (docs/offline.md, "Connect timeout"). Over https, which every
/// server behind `tailscale serve` is, reqwest counts the TLS handshake (and
/// the DNS lookup) in the connect timeout: a handshake that takes longer
/// than 2 s (one lost SYN costs 1 s, then two round trips on a slow mobile
/// link or a relayed tailnet path) is an unreachable server, and the
/// background check uses the same pool, so it cannot find it again either.
/// A listener that takes connections and never answers stands for the slow
/// handshake here: over http the read waits for `timeout_secs`, over https
/// it gives up after 2 s. Ignored: a decision for the owner (and 6 s of
/// real time); run it with `--ignored`.
#[tokio::test]
#[ignore = "the TLS handshake counts in the 2 s connect timeout: owner's decision"]
async fn reads_over_https_get_timeout_secs_for_the_handshake() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let timeout = Duration::from_secs(4);
    let waited = |scheme: &'static str| async move {
        let client = ApiClient::new(&format!("{scheme}://{addr}"), timeout)
            .unwrap()
            .with_fail_fast();
        let start = std::time::Instant::now();
        let err = client.health().await.unwrap_err();
        assert!(err.is_offline(), "{err}");
        start.elapsed()
    };
    let http = waited("http").await;
    assert!(http >= timeout, "http: {http:?}");
    let https = waited("https").await;
    assert!(
        https >= timeout - Duration::from_millis(500),
        "https: the read gave up during the TLS handshake after {https:?} (http waited {http:?})"
    );
}
