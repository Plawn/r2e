# Declarative Consumers

Instead of manually calling `event_bus.subscribe()`, use `#[consumer]` for declarative event handling within a controller. Consumers are auto-discovered and registered — no manual wiring needed.

## Defining a consumer

```rust
#[derive(Controller)]
#[controller(state = AppState)]
pub struct UserEventConsumer {
    #[inject] event_bus: EventBus,
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

1. `#[consumer(bus = "event_bus")]` tells R2E which `EventBus` field to subscribe to
2. The event type is inferred from the parameter type (`Arc<UserCreatedEvent>`)
3. `register_controller::<UserEventConsumer>()` auto-discovers all `#[consumer]` methods
4. At runtime, the controller is constructed from state via `StatefulConstruct` and each consumer method is subscribed to its event type

Under the hood, the `#[routes]` macro generates a `consumers()` method on the `Controller` trait impl. When `register_controller()` is called, it invokes `consumers()` and subscribes each returned closure to the appropriate bus.

## Requirements

- The controller must have an `EventBus` field (named in the `bus = "..."` attribute)
- The controller must **not** have struct-level `#[inject(identity)]` fields (consumers need `StatefulConstruct`)
- The consumer method must take `&self` and `Arc<EventType>`
- Consumer controllers don't need a `path` in `#[controller]`

## Registration

Register consumer controllers like any other controller:

```rust
AppBuilder::new()
    .provide(event_bus)
    .build_state::<AppState, _, _>()
    .await
    .register_controller::<UserEventConsumer>()
    // ...
```

Registration happens once at startup. All consumer methods on the controller are subscribed in a single pass.

## Mixed controllers

A controller can have both HTTP routes and consumers. This is common when a feature needs both a REST API and event-driven side-effects:

```rust
#[derive(Controller)]
#[controller(path = "/notifications", state = AppState)]
pub struct NotificationController {
    #[inject] event_bus: EventBus,
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
#[derive(Controller)]
#[controller(state = AppState)]
pub struct OrderConsumer {
    #[inject] event_bus: EventBus,
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

## Consumers vs manual subscribe

| | `#[consumer]` | `bus.subscribe()` |
|-|--------------|-------------------|
| Wiring | Automatic via `register_controller()` | Manual at startup |
| Access to services | Via `#[inject]` fields | Must capture in closure |
| Discovery | Declarative, visible in code structure | Scattered across init code |
| Identity access | No (requires `StatefulConstruct`) | No (no HTTP context) |

Use `#[consumer]` for standard application event handling. Use manual `subscribe()` when you need to register handlers dynamically or outside of a controller context.

## Limitations

- **No unsubscribe** — once registered, a consumer listens for the lifetime of the application. There is no mechanism to remove a subscription.
- **No ordering guarantees** — when multiple handlers are subscribed to the same event type, they run concurrently. Execution order is not guaranteed.
- **No identity** — consumers run outside an HTTP context, so `#[inject(identity)]` is not available at the struct level. Use the [mixed controller pattern](../core-concepts/controllers.md#mixed-controllers-param-level-identity) if you need both consumers and authenticated endpoints on the same controller.
