use crate::models::{Order, PlaceOrderRequest};
use crate::services::OrderService;
use r2e::prelude::*;

/// Controller for the "orders" feature module.
///
/// It injects `OrderService`, which in turn injects `UserService` — a bean
/// exported by `UserModule` and imported here via `imports(module(UserModule))`
/// (see `OrderModule` in `app.rs`). Placing an order therefore reaches across
/// module boundaries into `UserService`.
///
/// Kept public (no struct-level identity) to keep the cross-module demo focused.
#[controller(path = "/orders")]
pub struct OrderController {
    #[inject]
    order_service: OrderService,
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<Order>> {
        Json(self.order_service.list().await)
    }

    #[post("/")]
    async fn place(&self, Json(body): Json<PlaceOrderRequest>) -> Result<Json<Order>, HttpError> {
        match self.order_service.place_order(body.user_id, body.item).await {
            Some(order) => Ok(Json(order)),
            None => Err(HttpError::NotFound("User not found".into())),
        }
    }
}
