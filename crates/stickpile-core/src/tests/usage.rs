use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::cache::Cache;
use crate::mock::dead_url;
use crate::usage::{copy_event, UsageQueue};
use crate::CoreError;

#[test]
fn queue_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let (_server, items) = seeded_server();
    let ids: Vec<String> = {
        let cache = Arc::new(Cache::open(dir.path(), 16).unwrap());
        let q = UsageQueue::new(cache);
        let evs: Vec<_> = items.iter().map(|i| copy_event(i, None, None, "test")).collect();
        for e in &evs {
            q.push(e).unwrap();
        }
        q.push(&evs[0]).unwrap(); // duplicate id ignored
        evs.into_iter().map(|e| e.id).collect()
    };
    let cache = Arc::new(Cache::open(dir.path(), 16).unwrap());
    let q = UsageQueue::new(cache);
    let pending: Vec<String> = q.pending().unwrap().into_iter().map(|e| e.id).collect();
    assert_eq!(pending, ids, "same events, in order");
}

#[tokio::test]
async fn flush_is_idempotent_and_drops_answered_events() {
    let (server, items) = seeded_server();
    let client = client_for(&server.url, Some(server.state().issue_token("alice", "t")));
    let dir = tempfile::tempdir().unwrap();
    let q = UsageQueue::new(Arc::new(Cache::open(dir.path(), 16).unwrap()));
    let evs: Vec<_> = items[..3]
        .iter()
        .map(|i| copy_event(i, Some(stickpile_api::TargetApp::Discord), None, "test"))
        .collect();
    for e in &evs {
        q.push(e).unwrap();
    }
    let mut gone = items[0].clone();
    gone.id = "itemthatdoesnotexist234567".into();
    let ghost = copy_event(&gone, None, None, "test");
    q.push(&ghost).unwrap();

    let r = q.flush(&client).await.unwrap();
    assert_eq!(r.accepted, 3);
    assert_eq!(r.rejected, vec![ghost.id.clone()]);
    assert_eq!(r.remaining, 0);
    assert!(q.is_empty().unwrap());

    // Simulate a crash after the server accepted but before the local delete.
    for e in &evs {
        q.push(e).unwrap();
    }
    let r = q.flush(&client).await.unwrap();
    assert_eq!(r.accepted, 0);
    assert_eq!(r.duplicates, 3);
    assert!(q.is_empty().unwrap());
    assert_eq!(server.state().usage.len(), 3);
    assert_eq!(server.state().items.iter().map(|i| i.item.copies).sum::<u64>(), 3);

    // Empty queue: no request at all.
    let before = server.state().usage_requests;
    q.flush(&client).await.unwrap();
    assert_eq!(server.state().usage_requests, before);
}

#[tokio::test]
async fn failures_keep_events_queued() {
    let (server, items) = seeded_server();
    let dir = tempfile::tempdir().unwrap();
    let q = UsageQueue::new(Arc::new(Cache::open(dir.path(), 16).unwrap()));
    q.push(&copy_event(&items[0], None, None, "t")).unwrap();

    let offline = client_for(&dead_url(), Some("spk_x".into()));
    assert!(q.flush(&offline).await.unwrap_err().is_offline());
    assert_eq!(q.len().unwrap(), 1);

    let logged_out = client_for(&server.url, None);
    assert!(matches!(q.flush(&logged_out).await, Err(CoreError::NotLoggedIn)));
    assert_eq!(q.len().unwrap(), 1);

    let client = client_for(&server.url, Some(server.state().issue_token("alice", "t")));
    server.state().fail_usage = Some(503);
    assert_eq!(q.flush(&client).await.unwrap_err().status(), Some(503));
    assert_eq!(q.len().unwrap(), 1);

    server.state().fail_usage = Some(400);
    let r = q.flush(&client).await.unwrap();
    assert_eq!(r.dropped, 1, "malformed batches are not retried forever");
    assert_eq!(q.len().unwrap(), 0);
}

#[tokio::test]
async fn batches_and_single_flight() {
    let (server, items) = seeded_server();
    let client = client_for(&server.url, Some(server.state().issue_token("alice", "t")));
    let dir = tempfile::tempdir().unwrap();
    let q = Arc::new(UsageQueue::new(Arc::new(Cache::open(dir.path(), 16).unwrap())));
    for i in 0..250 {
        q.push(&copy_event(&items[i % 3], None, None, "t")).unwrap();
    }
    server.state().usage_delay = Some(Duration::from_millis(150));
    let (a, b) = tokio::join!(q.flush(&client), async {
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(q.is_flushing());
        q.flush(&client).await
    });
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.accepted + b.accepted, 250);
    assert_eq!(b.sent, 0, "second caller waited and found the queue drained");
    assert_eq!(server.state().usage_requests, 3, "100 + 100 + 50");
    assert_eq!(server.state().usage.len(), 250);
}
