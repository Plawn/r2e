# Declarative Consumers

Instead of manually calling `event_bus.subscribe()`, use `#[consumer]` for declarative event handling within controllers and beans. Consumers are auto-discovered and registered — no manual wiring needed.

## Controller consumers

```rust
#[controller]
pub struct UserEventConsumer {
    #[inject] event_bus: LocalEventBus,
}

#[routes]
impl UserEventConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        tracing::info!(
            user_id = event.user_id,
            "User created: {} ({})",
            event.name,
            event.email,
        );
    }

    #[consumer(bus = "event_bus")]
    async fn on_order_placed(&self, event: Arc<OrderPlacedEvent>) {
        tracing::info!(order_id = event.order_id, "Order placed: ${}", event.total);
    }
}
```

The `bus = "event_bus"` attribute refers to the **field name** on the controller struct.

## How it works

1. `#[consumer(bus = "event_bus")]` tells R2E which event bus field to subscribe to
2. The event type is inferred from the parameter type (`Arc<UserCreatedEvent>`)
3. `register_controller::<UserEventConsumer>()` auto-discovers all `#[consumer]` methods
4. At registration, the controller core is constructed from the bean graph via `ContextConstruct` (each `#[inject]` field resolved by type) and each consumer method is subscribed to its event type

Under the hood, the `#[routes]` macro generates a `register_consumers()` method on the `Controller` trait impl. When `register_controller()` is called, it invokes this and subscribes each handler via `EventBus::subscribe()` (UFCS).

## Requirements

- The controller must have a field implementing `EventBus` (named in the `bus = "..."` attribute)
- Consumer methods run on the controller core (built from the bean graph via `ContextConstruct`) and so cannot access request-scoped fields — reading `#[inject(identity)]` / `#[inject(request)]` inside a consumer is a compile error. `ContextConstruct` is generated for **every** controller core (identity and request-scoped fields are stripped onto the per-request façade), so a controller may freely combine struct-level identity for its authenticated HTTP routes with `#[consumer]` methods — the consumer simply doesn't see the identity. Consumer methods use only core (`#[inject]` / `#[config]`) fields.
- The consumer method must take `&self` and `Arc<EventType>`
- Consumer controllers don't need a `path` in `#[controller]`

## Registration

Register consumer controllers like any other controller:

```rust
AppBuilder::new()
    .provide(event_bus)
    .build_state()
    .await
    .register_controller::<UserEventConsumer>()
    // ...
```

Registration happens once at startup. All consumer methods on the controller are subscribed in a single pass.

## Mixed controllers

A controller can have both HTTP routes and consumers. This is common when a feature needs both a REST API and event-driven side-effects:

```rust
#[controller(path = "/notifications")]
pub struct NotificationController {
    #[inject] event_bus: LocalEventBus,
    #[inject] notification_service: NotificationService,
}

#[routes]
impl NotificationController {
    // HTTP route
    #[get("/")]
    async fn list(&self) -> Json<Vec<Notification>> {
        Json(self.notification_service.list().await)
    }

    // Event consumer
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        self.notification_service.send(
            &event.email,
            "Welcome!",
        ).await;
    }
}
```

## Using injected services

Consumer methods have access to all `#[inject]` fields on the controller, just like HTTP handlers. This makes it easy to reuse services:

```rust
#[controller]
pub struct OrderConsumer {
    #[inject] event_bus: LocalEventBus,
    #[inject] inventory_service: InventoryService,
    #[inject] notification_service: NotificationService,
}

#[routes]
impl OrderConsumer {
    #[consumer(bus = "event_bus")]
    async fn on_order_placed(&self, event: Arc<OrderPlacedEvent>) {
        // Use injected services
        self.inventory_service.reserve(event.order_id).await;
        self.notification_service.send_order_confirmation(event.order_id).await;
    }
}
```

## Error handling

Consumer handlers return `()` — there is no built-in error propagation for event handlers. Handle errors within the handler:

```rust
#[consumer(bus = "event_bus")]
async fn on_order_placed(&self, event: Arc<OrderPlacedEvent>) {
    if let Err(e) = self.inventory_service.reserve(event.order_id).await {
        tracing::error!(order_id = event.order_id, error = %e, "Failed to reserve inventory");
        // Optionally: emit a failure event, push to a retry queue, etc.
    }
}
```

If a handler panics, the panic is caught by the Tokio runtime. Other handlers for the same event continue running, and the bus remains operational. See [Panic isolation](./event-bus.md#panic-isolation).

## Bean consumers

Beans can also declare `#[consumer]` methods using the same syntax. This avoids creating a controller just for event handling:

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
        self.mailer.send_welcome(&event.email).await;
    }
}
```

The `#[bean]` macro generates an `EventSubscriber` impl when `#[consumer]` methods are present, and registration is automatic — `.register::<S>()` alone is enough:

```rust
AppBuilder::new()
    .provide(event_bus)
    .register::<NotificationService>()
    .build_state()
    .await
    // ...
```

`build_state()` collects the subscription (resolving `S` from the bean graph by type) and it runs at server startup, at the same point controller `#[consumer]` methods subscribe. Bean consumers capture `self` directly, so no per-event construction happens.

> **Note:** the auto-collection hook only runs for **registered** beans. An instance deposited via `.provide(..)` does not auto-subscribe — register the type instead, or wire it manually with `add_consumer_registration`.

## Multiple buses

Both controllers and beans support multiple event bus fields of different types. Each `#[consumer]` references a specific field by name:

```rust
#[bean]
impl NotificationService {
    pub fn new(local_bus: LocalEventBus, kafka_bus: KafkaEventBus, mailer: Mailer) -> Self {
        Self { local_bus, kafka_bus, mailer }
    }

    #[consumer(bus = "local_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        self.mailer.send_welcome(&event.email).await;
    }

    #[consumer(bus = "kafka_bus")]
    async fn on_order_placed(&self, event: Arc<OrderPlacedEvent>) {
        self.mailer.send_receipt(&event.order_id).await;
    }
}
```

The generated code calls `EventBus::subscribe()` via UFCS on each field — fully monomorphized per bus type, zero dynamic dispatch.

## Consumers vs manual subscribe

| | Controller `#[consumer]` | Bean `#[consumer]` | `bus.subscribe()` |
|-|-------------------------|--------------------|--------------------|
| Wiring | `register_controller()` | Automatic (`.register::<S>()`) | Manual at startup |
| Access to services | Via `#[inject]` fields | Via struct fields | Must capture in closure |
| Construction | Once at registration (from bean graph) | None (self captured) | N/A |
| Discovery | Declarative | Declarative | Scattered across init code |
| Identity access | No (`ContextConstruct`) | No | No (no HTTP context) |

Use `#[consumer]` on **beans** for services that primarily handle events. Use `#[consumer]` on **controllers** when you need both HTTP routes and event handlers on the same type. Use manual `subscribe()` when you need to register handlers dynamically.

## Limitations

- **No unsubscribe** — once registered, a consumer listens for the lifetime of the application. There is no mechanism to remove a subscription.
- **No ordering guarantees** — when multiple handlers are subscribed to the same event type, they run concurrently. Execution order is not guaranteed.
- **No identity** — consumers run outside an HTTP context, so identity and request-scoped fields (`#[inject(identity)]` / `#[inject(request)]`) are unreachable from a consumer body (a compile error). The controller may still declare struct-level identity for its authenticated HTTP routes — the consumer just cannot read it.
