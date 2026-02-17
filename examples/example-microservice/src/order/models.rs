use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Order {
    pub id: u64,
    pub product_id: u64,
    pub product_name: String,
    pub quantity: u32,
    pub total_price: f64,
    pub status: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateOrderRequest {
    pub product_id: u64,
    pub quantity: u32,
}
