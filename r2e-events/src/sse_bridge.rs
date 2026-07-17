//! EventBus ↔ SSE bridge: forward emitted events into an [`SseTopic`].
//!
//! The bridge turns real-time fan-out into zero-liaison code: services
//! `bus.emit(event)` as usual, and every bridged event type is broadcast to
//! the SSE clients subscribed to its [`SseTopic<E>`]. With a distributed
//! EventBus backend, SSE fan-out works across instances for free.
//!
//! ```ignore
//! use r2e_events::sse_bridge::SseBridgeExt;
//!
//! AppBuilder::new()
//!     .provide(LocalEventBus::new())
//!     .provide(SseTopic::<SyncStatus>::new(128))
//!     .register::<SyncController>()
//!     .build_state()
//!     .await
//!     .register_controller::<SyncController>()
//!     .bridge_sse::<LocalEventBus, SyncStatus>()
//!     .serve_auto()
//!     .await
//! ```
//!
//! ```ignore
//! #[controller(path = "/sync")]
//! struct SyncController {
//!     #[inject] topic: SseTopic<SyncStatus>,
//! }
//!
//! #[routes]
//! impl SyncController {
//!     #[sse("/status")]
//!     async fn status(&self) -> impl Stream<Item = Result<SseEvent, Infallible>> {
//!         self.topic.subscribe()
//!     }
//! }
//! ```

use r2e_core::sse::SseTopic;
use serde::{de::DeserializeOwned, Serialize};

use crate::{EventBus, EventBusError, HandlerResult, SubscriptionHandle};

/// Subscribe a forwarding handler on `bus`: every event of type `E` is
/// serialized to JSON and broadcast on `topic` under its SSE event name.
///
/// Publishing to a topic with no connected SSE clients is a no-op;
/// serialization failures are logged and the event is still acked (a
/// broadcast is fire-and-forget — there is no meaningful retry target).
///
/// This is the manual entry point; app code normally uses
/// [`SseBridgeExt::bridge_sse`] instead.
pub async fn bridge_event_to_sse<B, E>(
    bus: &B,
    topic: SseTopic<E>,
) -> Result<SubscriptionHandle, EventBusError>
where
    B: EventBus,
    E: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    bus.subscribe::<E, _, _>(move |envelope| {
        let topic = topic.clone();
        async move {
            if let Err(err) = topic.publish(&envelope.event) {
                tracing::warn!(
                    event = topic.event_name(),
                    error = %err,
                    "EventBus→SSE bridge: failed to serialize event"
                );
            }
            HandlerResult::Ack
        }
    })
    .await
}

/// Builder extension wiring the EventBus↔SSE bridge.
///
/// Available on the post-`build_state()` [`AppBuilder`](r2e_core::AppBuilder).
/// Both the bus and the topic must have been provided before `build_state()`.
pub trait SseBridgeExt<T>: Sized
where
    T: Clone + Send + Sync + 'static,
{
    /// Bridge events of type `E` emitted on the bus bean `B` into the
    /// provided `SseTopic<E>` bean.
    ///
    /// The forwarding subscription is registered during server startup, at
    /// the same point `#[consumer]` methods subscribe.
    ///
    /// # Panics
    ///
    /// Panics if `B` or `SseTopic<E>` was not provided/registered on the
    /// builder before `build_state()`.
    fn bridge_sse<B, E>(self) -> Self
    where
        B: EventBus,
        E: Serialize + DeserializeOwned + Send + Sync + 'static;
}

impl<T> SseBridgeExt<T> for r2e_core::AppBuilder<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn bridge_sse<B, E>(self) -> Self
    where
        B: EventBus,
        E: Serialize + DeserializeOwned + Send + Sync + 'static,
    {
        let bus = self.bean_context().try_get::<B>().unwrap_or_else(|| {
            panic!(
                "bridge_sse::<{bus}, {event}>(): event bus bean not found in the resolved \
                 graph — add `.provide(...)` before `build_state()`",
                bus = std::any::type_name::<B>(),
                event = std::any::type_name::<E>()
            )
        });
        let topic = self
            .bean_context()
            .try_get::<SseTopic<E>>()
            .unwrap_or_else(|| {
                panic!(
                    "bridge_sse::<{bus}, {event}>(): SseTopic<{event}> bean not found in the \
                     resolved graph — add `.provide(SseTopic::<{event}>::new(capacity))` before \
                     `build_state()`",
                    bus = std::any::type_name::<B>(),
                    event = std::any::type_name::<E>()
                )
            });
        self.add_consumer_registration(move |_state| async move {
            if let Err(err) = bridge_event_to_sse(&bus, topic).await {
                tracing::error!(
                    event = std::any::type_name::<E>(),
                    error = %err,
                    "EventBus→SSE bridge: failed to subscribe"
                );
            }
        })
    }
}
