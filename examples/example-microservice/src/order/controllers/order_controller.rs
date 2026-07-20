use r2e::prelude::*;

use crate::order::models::{CreateOrderRequest, Order};
use crate::order::services::OrderService;

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
    async fn create(&self, Json(body): Json<CreateOrderRequest>) -> Result<Json<Order>, HttpError> {
        let order = self.order_service.create(body).await?;
        Ok(Json(order))
    }
}
