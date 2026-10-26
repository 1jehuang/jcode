//! Pending JSON-RPC request slots.
//!
//! Two transports correlate replies out of band: stdio reads them from the
//! child's stdout, and the legacy SSE transport reads them from a long-lived
//! event stream. Both need the same three behaviors, so they live here rather
//! than being written twice and drifting apart:
//!
//! * remove a slot when the waiting future goes away (a cancelled call or an
//!   elapsed reply deadline), otherwise the map grows for the life of the
//!   connection,
//! * hand a response to its waiter by id,
//! * wake every waiter when the channel that would answer them dies.

use super::protocol::JsonRpcResponse;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

/// Requests awaiting a reply, keyed by JSON-RPC id.
pub(super) type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>;

pub(super) fn new_pending() -> PendingMap {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Deliver a response to its waiter, if one is still listening.
pub(super) async fn resolve(pending: &PendingMap, response: JsonRpcResponse) {
    let Some(id) = response.id else {
        return;
    };
    let sender = pending.lock().await.remove(&id);
    if let Some(sender) = sender {
        let _ = sender.send(response);
    }
}

/// Wake every waiter with an error because nothing can answer them anymore.
///
/// Dropping the senders makes each `recv` fail immediately instead of leaving
/// the caller to sit out its full reply deadline after the stream has died.
pub(super) async fn fail_all(pending: &PendingMap) {
    pending.lock().await.clear();
}

/// Removes a request's slot when the waiting future is dropped, whether it
/// completed, was cancelled, or hit the reply deadline.
pub(super) struct PendingGuard {
    pending: PendingMap,
    id: u64,
    armed: bool,
}

impl PendingGuard {
    pub(super) fn new(pending: PendingMap, id: u64) -> Self {
        Self {
            pending,
            id,
            armed: true,
        }
    }

    /// Remove the slot now rather than on drop.
    ///
    /// Drop cannot await, so it falls back to a detached removal when the map
    /// is contended. A caller that immediately re-registers the same id (a
    /// transport retrying a request after re-authorizing) would then race that
    /// detached task, which could delete the new slot and strand the retry
    /// until its deadline. Cancelling explicitly keeps the ordering exact.
    pub(super) async fn cancel(mut self) {
        self.armed = false;
        self.pending.lock().await.remove(&self.id);
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Drop can run outside an async context, so reach for the lock without
        // awaiting and fall back to a detached cleanup when it is contended.
        if let Ok(mut pending) = self.pending.try_lock() {
            pending.remove(&self.id);
            return;
        }
        let pending = Arc::clone(&self.pending);
        let id = self.id;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                pending.lock().await.remove(&id);
            });
        }
    }
}

/// A registered request, held from just before the message is sent until its
/// reply arrives or the caller gives up.
pub(super) struct PendingRequest {
    guard: PendingGuard,
    receiver: oneshot::Receiver<JsonRpcResponse>,
}

impl PendingRequest {
    /// Register `id` before sending, so a reply that arrives immediately still
    /// finds a waiter.
    pub(super) async fn register(pending: &PendingMap, id: u64) -> Self {
        let (sender, receiver) = oneshot::channel();
        pending.lock().await.insert(id, sender);
        Self {
            guard: PendingGuard::new(Arc::clone(pending), id),
            receiver,
        }
    }

    /// Wait for the reply. The caller owns the deadline, and dropping this
    /// future releases the slot.
    pub(super) async fn recv(self) -> anyhow::Result<JsonRpcResponse> {
        let Self { guard, receiver } = self;
        let response = receiver.await;
        drop(guard);
        response.map_err(|_| anyhow::anyhow!("MCP server closed before replying"))
    }

    /// Give up on this registration before retrying or failing.
    pub(super) async fn cancel(self) {
        let Self { guard, receiver } = self;
        drop(receiver);
        guard.cancel().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request whose waiter is cancelled (the reply deadline firing, for
    /// instance) must not leave its slot behind: the map lives as long as the
    /// connection, so one leaked entry per timed-out call grows without bound.
    #[tokio::test]
    async fn a_cancelled_request_does_not_leak_its_pending_slot() {
        let pending = new_pending();

        {
            let (sender, receiver) = oneshot::channel::<JsonRpcResponse>();
            pending.lock().await.insert(7, sender);
            let _guard = PendingGuard::new(Arc::clone(&pending), 7);
            assert_eq!(pending.lock().await.len(), 1);

            // Nothing ever answers, so the deadline elapses and the waiting
            // future is dropped.
            let timed_out =
                tokio::time::timeout(std::time::Duration::from_millis(20), receiver).await;
            assert!(timed_out.is_err(), "the request should time out");
        }

        assert!(
            pending.lock().await.is_empty(),
            "a timed-out request must not stay in the pending map"
        );
    }

    #[tokio::test]
    async fn a_response_reaches_the_waiter_that_asked_for_it() {
        let pending = new_pending();
        let (sender, receiver) = oneshot::channel();
        pending.lock().await.insert(4, sender);

        resolve(
            &pending,
            serde_json::from_value(serde_json::json!({
                "jsonrpc": "2.0", "id": 4, "result": {"ok": true}
            }))
            .expect("response"),
        )
        .await;

        assert_eq!(receiver.await.expect("delivered").id, Some(4));
        assert!(pending.lock().await.is_empty(), "the slot must be consumed");
    }

    /// When the answering channel dies, waiters must learn immediately rather
    /// than waiting out a 30s reply deadline that can never be met.
    #[tokio::test]
    async fn closing_the_channel_wakes_every_waiter() {
        let pending = new_pending();
        let (sender, receiver) = oneshot::channel::<JsonRpcResponse>();
        pending.lock().await.insert(1, sender);

        fail_all(&pending).await;

        assert!(receiver.await.is_err(), "the waiter must be woken");
    }
}
