//! Offline usage-event queue ("item X was copied at time T").
//!
//! Events get a client-generated id ([`crate::new_id`], 26 lowercase base32
//! chars) when they are recorded, are persisted in the cache database, and are
//! uploaded in batches to `POST /usage`. Uploads are idempotent: the server
//! deduplicates by id, so an event is removed from the queue once the server
//! answered for its batch, whether it was accepted, a duplicate or rejected
//! (item gone or not visible). Transport failures and server errors keep the
//! events queued. Only one flush runs at a time per queue; concurrent callers
//! wait for it and then find the queue drained.

use std::sync::Arc;

use stickpile_api::{ItemSummary, TargetApp, UsageBatch, UsageEvent, Variant};

use crate::cache::Cache;
use crate::client::ApiClient;
use crate::{CoreError, Result};

/// Events per `POST /usage` request.
pub const BATCH_SIZE: usize = 100;

/// Result of [`UsageQueue::flush`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlushReport {
    /// Events sent in batches the server answered.
    pub sent: usize,
    pub accepted: u64,
    pub duplicates: u64,
    pub rejected: Vec<String>,
    /// Malformed batches the server refused with 400/422 (dropped).
    pub dropped: usize,
    /// Still queued afterwards.
    pub remaining: usize,
}

/// A usage event for a copy that just happened.
pub fn copy_event(item: &ItemSummary, target: Option<TargetApp>, variant: Option<Variant>, client: &str) -> UsageEvent {
    UsageEvent {
        id: crate::new_id(),
        item_id: item.id.clone(),
        copied_at: crate::now_unix(),
        target,
        variant,
        client: Some(client.to_string()),
    }
}

/// Persistent queue on top of the cache database.
#[derive(Debug)]
pub struct UsageQueue {
    cache: Arc<Cache>,
    in_flight: tokio::sync::Mutex<()>,
}

impl UsageQueue {
    pub fn new(cache: Arc<Cache>) -> UsageQueue {
        UsageQueue {
            cache,
            in_flight: tokio::sync::Mutex::new(()),
        }
    }

    pub fn push(&self, event: &UsageEvent) -> Result<()> {
        self.cache.queue_push(event)
    }

    pub fn pending(&self) -> Result<Vec<UsageEvent>> {
        self.cache.queue_peek(usize::MAX >> 1)
    }

    pub fn len(&self) -> Result<usize> {
        self.cache.queue_len()
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }

    /// True while a flush is running.
    pub fn is_flushing(&self) -> bool {
        self.in_flight.try_lock().is_err()
    }

    /// Upload queued events in batches until the queue is empty or a batch
    /// fails. An empty queue is not an error even when offline or logged out.
    pub async fn flush(&self, client: &ApiClient) -> Result<FlushReport> {
        let _guard = self.in_flight.lock().await;
        let mut report = FlushReport::default();
        loop {
            let batch = self.cache.queue_peek(BATCH_SIZE)?;
            if batch.is_empty() {
                break;
            }
            let ids: Vec<String> = batch.iter().map(|e| e.id.clone()).collect();
            match client.usage(&UsageBatch { events: batch }).await {
                Ok(ack) => {
                    report.sent += ids.len();
                    report.accepted += ack.accepted;
                    report.duplicates += ack.duplicates;
                    report.rejected.extend(ack.rejected);
                    self.cache.queue_remove(&ids)?;
                }
                Err(CoreError::Api {
                    status: 400 | 422,
                    error,
                }) => {
                    tracing::warn!(
                        "server refused a usage batch ({}); dropping {} events",
                        error.message,
                        ids.len()
                    );
                    report.dropped += ids.len();
                    self.cache.queue_remove(&ids)?;
                }
                Err(e) => {
                    report.remaining = self.cache.queue_len()?;
                    tracing::debug!(remaining = report.remaining, "usage flush stopped: {e}");
                    return Err(e);
                }
            }
        }
        report.remaining = self.cache.queue_len()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ids_are_26_lowercase_base32() {
        let a = crate::new_id();
        let b = crate::new_id();
        assert_eq!(a.len(), 26);
        assert_ne!(a, b);
        assert!(
            a.chars().all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)),
            "{a}"
        );
    }
}
