use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Product information — shared contract between services.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProductInfo {
    pub id: u64,
    pub name: String,
    pub price: f64,
    pub stock: u32,
}

/// Availability check response.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AvailabilityResponse {
    pub product_id: u64,
    pub available: bool,
    pub stock: u32,
}
