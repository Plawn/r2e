//! SSE (Server-Sent Events) broadcaster for multi-client streaming.
//!
//! # Usage
//!
//! ```ignore
//! use quarlus_core::sse::SseBroadcaster;
//!
//! // Create a broadcaster (injectable via #[inject])
//! let broadcaster = SseBroadcaster::new(128);
//!
//! // In a handler — subscribe a client
//! let stream = broadcaster.subscribe();
//! Sse::new(stream).keep_alive(SseKeepAlive::default())
//!
//! // From anywhere — broadcast to all clients
//! broadcaster.send("hello").ok();
//! broadcaster.send_event("update", r#"{"count":42}"#).ok();
//! ```

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::response::sse::Event as SseEvent;
use tokio::sync::broadcast;

/// Message sent through the broadcast channel.
#[derive(Clone, Debug)]
pub struct SseMessage {
    /// Optional event type name.
    pub event: Option<String>,
    /// Event data payload.
    pub data: String,
}

/// Injectable SSE broadcaster for multi-client streaming.
///
/// Clone + Send + Sync — can be used as an `#[inject]` field on controllers.
#[derive(Clone)]
pub struct SseBroadcaster {
    tx: broadcast::Sender<SseMessage>,
}

impl SseBroadcaster {
    /// Create a new broadcaster with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Broadcast a data-only event to all subscribers.
    pub fn send(&self, data: impl Into<String>) -> Result<(), broadcast::error::SendError<SseMessage>> {
        self.tx.send(SseMessage {
            event: None,
            data: data.into(),
        })?;
        Ok(())
    }

    /// Broadcast a typed event to all subscribers.
    pub fn send_event(
        &self,
        event: &str,
        data: impl Into<String>,
    ) -> Result<(), broadcast::error::SendError<SseMessage>> {
        self.tx.send(SseMessage {
            event: Some(event.to_string()),
            data: data.into(),
        })?;
        Ok(())
    }

    /// Create a new subscription stream suitable for returning from an SSE handler.
    pub fn subscribe(&self) -> SseSubscription {
        SseSubscription {
            rx: self.tx.subscribe(),
        }
    }
}

fn msg_to_event(msg: SseMessage) -> SseEvent {
    let mut event = SseEvent::default().data(msg.data);
    if let Some(ref name) = msg.event {
        event = event.event(name);
    }
    event
}

/// A subscription stream that yields SSE events.
///
/// Implements `Stream<Item = Result<SseEvent, Infallible>>` — ready to pass
/// to `Sse::new()`.
pub struct SseSubscription {
    rx: broadcast::Receiver<SseMessage>,
}

impl futures_core::Stream for SseSubscription {
    type Item = Result<SseEvent, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Drain any ready messages first via try_recv (non-blocking).
        loop {
            match self.rx.try_recv() {
                Ok(msg) => return Poll::Ready(Some(Ok(msg_to_event(msg)))),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Closed) => return Poll::Ready(None),
                Err(broadcast::error::TryRecvError::Empty) => break,
            }
        }

        // Nothing ready — register the waker by polling recv().
        // We use a boxed future to safely pin it within this single poll call.
        // Because broadcast::Receiver::recv() is cancel-safe, dropping the
        // future between polls does not lose messages. The receiver's internal
        // cursor only advances when a message is successfully read.
        let rx = &mut self.rx;
        let mut recv_fut = Box::pin(rx.recv());
        match recv_fut.as_mut().poll(cx) {
            Poll::Ready(Ok(msg)) => Poll::Ready(Some(Ok(msg_to_event(msg)))),
            Poll::Ready(Err(broadcast::error::RecvError::Closed)) => Poll::Ready(None),
            Poll::Ready(Err(broadcast::error::RecvError::Lagged(_))) => {
                // Waker is already registered; try_recv on next poll
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
