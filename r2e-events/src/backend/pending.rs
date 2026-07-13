//! Correlation map for in-flight request-reply calls on distributed backends.
//!
//! A requester registers a pending entry (keyed by a u128 correlation id drawn
//! from the shared `event_id` scheme), publishes the request carrying that id,
//! and awaits the returned [`oneshot::Receiver`]. The backend's reply consumer
//! matches an incoming reply's correlation id and calls [`complete`] to hand
//! the reply bytes (or an error) to the waiter. If the requester times out and
//! drops its [`PendingGuard`], the entry is removed so the map never leaks.
//!
//! [`complete`]: PendingRequests::complete

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::sync::oneshot;

use super::metadata_codec::ReplyHeaders;
use crate::EventBusError;

/// The outcome delivered to a waiting requester: the raw reply payload bytes on
/// success, or an [`EventBusError`] (e.g. [`EventBusError::Remote`]).
pub type ReplyResult = Result<Vec<u8>, EventBusError>;

/// Correlation map from request id → the waiting requester's reply channel.
#[derive(Default)]
pub struct PendingRequests {
    map: Mutex<HashMap<u128, oneshot::Sender<ReplyResult>>>,
}

impl PendingRequests {
    /// Create an empty correlation map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new pending request.
    ///
    /// Allocates a fresh correlation id (from the shared `event_id` scheme) and
    /// returns `(id, guard, receiver)`: send the request tagged with `id`, await
    /// `receiver` for the reply, and keep `guard` alive until then — dropping it
    /// (e.g. on timeout) removes the entry from the map.
    pub fn register(self: &Arc<Self>) -> (u128, PendingGuard, oneshot::Receiver<ReplyResult>) {
        let id = crate::next_event_id();
        let (tx, rx) = oneshot::channel();
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        let guard = PendingGuard {
            pending: self.clone(),
            id,
        };
        (id, guard, rx)
    }

    /// Complete a pending request, delivering `result` to the waiter.
    ///
    /// A no-op if the id is unknown (already completed, or the requester timed
    /// out and removed its entry).
    pub fn complete(&self, id: u128, result: ReplyResult) {
        if let Some(tx) = self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
            let _ = tx.send(result);
        }
    }

    /// Complete the pending request identified by `headers.request_id` from a
    /// decoded reply message.
    ///
    /// Single-sources the Remote-vs-Ok decision every backend's reply consumer
    /// otherwise hand-rolls: a present `reply_error` becomes
    /// [`EventBusError::Remote`]; otherwise `payload` is delivered as the reply
    /// bytes. A no-op if the id is unknown (timed out / already completed).
    pub fn complete_reply(&self, headers: &ReplyHeaders, payload: Vec<u8>) {
        let result = match &headers.reply_error {
            Some(err) => Err(EventBusError::Remote(err.clone())),
            None => Ok(payload),
        };
        self.complete(headers.request_id, result);
    }

    /// Remove a pending entry without completing it (used by [`PendingGuard`]).
    fn remove(&self, id: u128) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    /// Number of currently in-flight requests.
    pub fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether there are no in-flight requests.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// RAII guard that removes its pending entry on drop.
///
/// Held by the requester while it awaits the reply. On timeout (or any early
/// return) the guard drops and evicts the correlation entry, so a late reply is
/// simply discarded instead of leaking a map slot.
pub struct PendingGuard {
    pending: Arc<PendingRequests>,
    id: u128,
}

impl PendingGuard {
    /// The correlation id this guard protects.
    pub fn id(&self) -> u128 {
        self.id
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending.remove(self.id);
    }
}

/// Await a registered request's reply, deserializing it into `Resp`.
///
/// The shared `request_with` tail: races the reply channel against a `timeout`
/// and a `shutdown` future. Both an elapsed timeout and a dropped reply sender
/// (the responder vanished without completing) map to
/// [`EventBusError::RequestTimeout`]; the `shutdown` future completing maps to
/// [`EventBusError::Shutdown`]. On a successful reply the bytes are decoded as
/// `Resp` ([`EventBusError::Serialization`] on failure).
///
/// The caller keeps the request's [`PendingGuard`] alive across this call and
/// drops it afterwards, so any late reply is discarded instead of leaking a
/// correlation-map slot.
pub async fn await_reply<Resp>(
    rx: oneshot::Receiver<ReplyResult>,
    timeout: Duration,
    shutdown: impl Future<Output = ()>,
) -> Result<Resp, EventBusError>
where
    Resp: DeserializeOwned,
{
    let result: ReplyResult = tokio::select! {
        r = rx => match r {
            Ok(reply) => reply,
            // Sender dropped without completing — treat as a timeout.
            Err(_) => Err(EventBusError::RequestTimeout),
        },
        _ = r2e_core::rt::sleep(timeout) => Err(EventBusError::RequestTimeout),
        _ = shutdown => Err(EventBusError::Shutdown),
    };

    let bytes = result?;
    serde_json::from_slice::<Resp>(&bytes)
        .map_err(|e| EventBusError::Serialization(e.to_string()))
}
