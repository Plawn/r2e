use std::sync::Arc;

use crate::models::{GreetReply, GreetRequest, UserCreatedEvent};
use r2e::prelude::*;

#[controller(path = "/events")]
pub struct UserEventConsumer {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl UserEventConsumer {
    /// Plain fan-out subscriber: `-> ()` return, registered via `subscribe`.
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreatedEvent>) {
        tracing::info!(
            user_id = event.user_id,
            name = %event.name,
            email = %event.email,
            "User created event received"
        );
    }

    /// Request-reply responder (Quarkus `@ConsumeEvent`-style): the non-`()`
    /// return type makes this a responder, registered via `EventBus::respond`.
    /// Its return value IS the reply delivered to `bus.request`.
    #[consumer(bus = "event_bus")]
    async fn greet(&self, req: Arc<GreetRequest>) -> GreetReply {
        GreetReply {
            message: format!("Hello, {}!", req.name),
        }
    }

    /// HTTP route that drives the request-reply flow: sends a `GreetRequest`
    /// over the bus and awaits the responder's `GreetReply`.
    #[get("/greet/{name}")]
    async fn greet_over_bus(
        &self,
        Path(name): Path<String>,
    ) -> Result<Json<GreetReply>, HttpError> {
        let reply: GreetReply = self
            .event_bus
            .request(GreetRequest { name })
            .await
            .map_err(|e| HttpError::internal(e.to_string()))?;
        Ok(Json(reply))
    }
}
