use std::sync::Arc;
use tokio::sync::RwLock;

use r2e::prelude::*;

use super::ProductClient;
use crate::models::{CreateOrderRequest, Order};

#[derive(Clone)]
pub struct OrderService {
    orders: Arc<RwLock<Vec<Order>>>,
    product_client: ProductClient,
}

#[bean]
impl OrderService {
    pub fn new(product_client: ProductClient) -> Self {
        Self {
            orders: Arc::new(RwLock::new(Vec::new())),
            product_client,
        }
    }

    pub async fn list(&self) -> Vec<Order> {
        self.orders.read().await.clone()
    }

    pub async fn create(&self, req: CreateOrderRequest) -> Result<Order, HttpError> {
        // Validate product exists by fetching from product service
        let product = self.product_client.get_product(req.product_id).await?;

        // Check availability
        let availability = self
            .product_client
            .check_availability(req.product_id)
            .await?;

        if !availability.available {
            return Err(HttpError::BadRequest(format!(
                "Product '{}' is out of stock",
                product.name
            )));
        }

        if availability.stock < req.quantity {
            return Err(HttpError::BadRequest(format!(
                "Insufficient stock for '{}': requested {}, available {}",
                product.name, req.quantity, availability.stock
            )));
        }

        let mut orders = self.orders.write().await;
        let id = orders.len() as u64 + 1;
        let order = Order {
            id,
            product_id: product.id,
            product_name: product.name,
            quantity: req.quantity,
            total_price: product.price * req.quantity as f64,
            status: "confirmed".into(),
        };
        orders.push(order.clone());
        Ok(order)
    }
}
