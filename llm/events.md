---
topic: events
features: events
tokens: ~1700
requires: di-beans
---

## Events (Pub/Sub)

### TL;DR

- Requires feature `events`; provide the bus with `b.provide(LocalEventBus::new())`. `EventBus` is a trait, so distributed backends (Kafka, Pulsar, RabbitMQ, Iggy) are drop-in.
- Event types derive `Serialize + Deserialize` even though `LocalEventBus` never actually serializes.
- Handlers receive `EventEnvelope<E>`; `event` and `metadata` are `Arc`s shared across the fan-out — clone (`(*envelope.metadata).clone()`) to own a metadata field.
- `emit` is fan-out with no reply and there is NO `emit_and_wait`; use `request`/`respond` when you need a result back.
- `emit_nowait` skips the broker ack and returns an `EmitReceipt` you may drop or `confirm().await?` later.
- At most one responder per request type per process; requests time out after 30s unless you pass `RequestOptions::new().with_timeout(..)` to `request_with`.
- `#[consumer(bus = "field")]` works on controllers and on beans; a method with a non-`()` return type is registered as a **responder** instead of a subscriber.
- Registration is automatic for a `.register::<T>()`d type — a bean deposited with `.provide(instance)` does NOT auto-subscribe.
- Consumers run on the controller core: they cannot read request-scoped fields or identity.
- `#[scheduled]` + `#[consumer]` on one method is a compile error, as is `#[intercept]` on a plain (non-route/scheduled/consumer) controller method.

Requires feature: `events`. `EventBus` is a trait; `LocalEventBus` is the
in-process default; distributed backends (Kafka, Pulsar, RabbitMQ, Iggy) are
drop-in. Events derive `Serialize + Deserialize` (LocalEventBus never actually
serializes).

A subscriber/responder handler receives an `EventEnvelope<E>` with
`event: Arc<E>` and `metadata: Arc<EventMetadata>` (both shared across a
fan-out, so all handlers of one emit see the same `event_id`). Fields deref
transparently for reads (`envelope.metadata.event_id`); to own a metadata field,
clone it (`(*envelope.metadata).clone()`). Emit APIs (`emit_with`,
`request_with` options) still take an owned `EventMetadata` — it is wrapped in an
`Arc` internally.

```rust
# fn __doc(b: AppBuilder) -> impl Sized {
b.provide(LocalEventBus::new())
# }
```

### Emitting

`emit` is fan-out publish/subscribe (Vert.x `publish` semantics): every
subscriber gets a copy; the emitter never waits for handlers and cannot
observe a reply. There is NO `emit_and_wait` — to get a result back, use
request-reply below.

```rust
# struct Publisher { event_bus: LocalEventBus }
# impl Publisher { async fn __doc(&self, event: UserCreatedEvent) -> Result<(), Box<dyn std::error::Error>> {
self.event_bus.emit(UserCreatedEvent { user_id: 1 }).await?;   // fan-out, awaits broker ack

// High-throughput fire-and-forget: don't wait for the broker ack.
let receipt: EmitReceipt = self.event_bus.emit_nowait(event).await?;
// drop the receipt, or `receipt.confirm().await?` later (batch via try_join_all)
# Ok(()) } }
```

### Request-reply — `request` / `respond`

Point-to-point (Vert.x `request` semantics): exactly one responder per request
type replies, with a timeout (30s default, `DEFAULT_REQUEST_TIMEOUT`).

```rust
# async fn __doc(event_bus: LocalEventBus, req: GreetRequest) -> Result<(), Box<dyn std::error::Error>> {
// Responder side — at most one responder per request type per process.
event_bus.respond(|envelope: EventEnvelope<GreetRequest>| async move {
    Ok::<_, String>(GreetReply { message: format!("Hello {}", envelope.event.name) })
}).await?;

// Requester side.
let reply: GreetReply = event_bus.request(GreetRequest { name: "Alice".into() }).await?;

// Explicit timeout.
let reply: GreetReply = event_bus
    .request_with(req, RequestOptions::new().with_timeout(Duration::from_secs(5)))
    .await?;
# Ok(()) }
```

Responder errors surface to the caller as `EventBusError::Remote`;
`NoResponder` is only detectable in-process (distributed backends manifest an
absent responder as `RequestTimeout`). All distributed backends support
request-reply via instance-private reply topics.

### Consuming — on controllers

```rust
#[controller]
pub struct UserEventConsumer {
    #[inject] event_bus: LocalEventBus,
}

#[routes]
impl UserEventConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        tracing::info!(user_id = event.user_id, "user created");
    }
}
// registered automatically by register_controller()
# fn main() {}
```

Consumers run on the controller core — they cannot access request-scoped
fields (identity), but work on any controller.

A `#[consumer]` method with a non-`()` return type is automatically registered
as a **responder**: its argument is the request, its return value is the reply
(pairs with `event_bus.request::<Req, Resp>(...)`).

Controller `#[consumer]` methods accept `#[intercept(...)]` (method-level, plus
an impl-level `#[intercept(...)]` on the `#[routes]` block that wraps every
`#[scheduled]`/`#[consumer]` method, impl-level outermost) — same as bean-level
interceptors, and covering both fan-out subscribers and responders. Direct
in-code calls on a registered core self-intercept too. A missing decorator bean
is a compile error at `.register_controller`; `#[scheduled]` + `#[consumer]` on
one method, or a stray `#[intercept]` on a plain (non-route/scheduled/consumer)
controller method, is also a compile error.

```rust
#[controller]
pub struct PingController {
    #[inject] event_bus: LocalEventBus,
}

#[routes]
#[intercept(Audit::spec("impl"))]            // wraps every consumer/scheduled method
impl PingController {
    #[consumer(bus = "event_bus")]
    #[intercept(Audit::spec("method"))]      // runs after the impl-level one
    async fn on_ping(&self, _e: Arc<Ping>) { tracing::debug!("ping"); }
}
# fn main() {}
```

### Consuming — on beans

```rust
#[derive(Clone)]
pub struct NotificationService {
    event_bus: LocalEventBus,
    mailer: Mailer,
}

#[bean]
impl NotificationService {
    pub fn new(event_bus: LocalEventBus, mailer: Mailer) -> Self {
        Self { event_bus, mailer }
    }

    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        self.mailer.send(&format!("user-{}", event.user_id)).await;
    }
}
// registration is automatic: .register::<NotificationService>() alone is enough —
// build_state() queues the subscription, run at server startup. NOTE: a bean
// deposited via .provide(instance) does NOT auto-subscribe; register the type,
// or wire manually with .add_consumer_registration(..).
```

### EventBus↔SSE bridge

```rust
# async fn __doc(b: AppBuilder) -> impl Sized {
b.provide(SseTopic::<UserCreatedEvent>::new(64).with_event_name("user_created"))
 .build_state().await
 .bridge_sse::<LocalEventBus, UserCreatedEvent>()   // bus.emit → SSE fan-out, zero liaison code
# }
```
