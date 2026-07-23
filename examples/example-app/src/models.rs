use garde::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}

#[derive(Deserialize, Validate, JsonSchema)]
pub struct CreateUserRequest {
    #[garde(length(min = 1, max = 100))]
    pub name: String,
    #[garde(email)]
    pub email: String,
}

/// An order placed by a user.
///
/// `user_name` is denormalized from the looked-up `User` at creation time —
/// it visibly proves that `OrderService` reached across into `UserService`
/// (imported via `imports(module(UserModule))`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Order {
    pub id: u64,
    pub user_id: u64,
    pub user_name: String,
    pub item: String,
}

/// Request body for placing an order.
#[derive(Deserialize, JsonSchema)]
pub struct PlaceOrderRequest {
    pub user_id: u64,
    pub item: String,
}

/// Database row used by the paginated data controller.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserEntity {
    pub id: i64,
    pub name: String,
    pub email: String,
}

/// Event emitted when a new user is created.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCreatedEvent {
    pub user_id: u64,
    pub name: String,
    pub email: String,
}

/// Point-to-point request asking for a greeting (request-reply demo).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GreetRequest {
    pub name: String,
}

/// Reply produced by the greeting responder.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GreetReply {
    pub message: String,
}
