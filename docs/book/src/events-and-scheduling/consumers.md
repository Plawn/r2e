# Declarative Consumers

Instead of manually calling `event_bus.subscribe()`, use `#[consumer]` for declarative event handling within a controller.

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

## How it works

1. `#[consumer(bus = "event_bus")]` tells R2E which `EventBus` field to subscribe to
2. The event type is inferred from the parameter type (`Arc<UserCreatedEvent>`)
3. `register_controller::<UserEventConsumer>()` auto-discovers and registers all consumer methods
4. At runtime, the controller is constructed from state via `StatefulConstruct` and the closure subscribes to events

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
    .build_state::<AppState, _>()
    .await
    .register_controller::<UserEventConsumer>()
    // ...
```

## Mixed controllers

A controller can have both HTTP routes and consumers:

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
