use std::sync::Arc;
use tokio::sync::RwLock;

use r2e::prelude::*;

use crate::shared::types::{AvailabilityResponse, ProductInfo};

#[derive(Clone)]
pub struct ProductService {
    products: Arc<RwLock<Vec<ProductInfo>>>,
}

#[bean]
impl ProductService {
    pub fn new() -> Self {
        Self {
            products: Arc::new(RwLock::new(vec![
                ProductInfo {
                    id: 1,
                    name: "Rust Programming Book".into(),
                    price: 39.99,
                    stock: 50,
                },
                ProductInfo {
                    id: 2,
                    name: "Ergonomic Keyboard".into(),
                    price: 149.99,
                    stock: 15,
                },
                ProductInfo {
                    id: 3,
                    name: "Monitor Stand".into(),
                    price: 79.99,
                    stock: 0, // out of stock
                },
            ])),
        }
    }

    pub async fn list(&self) -> Vec<ProductInfo> {
        self.products.read().await.clone()
    }

    pub async fn get_by_id(&self, id: u64) -> Option<ProductInfo> {
        self.products.read().await.iter().find(|p| p.id == id).cloned()
    }

    pub async fn check_availability(&self, id: u64) -> Option<AvailabilityResponse> {
        self.products
            .read()
            .await
            .iter()
            .find(|p| p.id == id)
            .map(|p| AvailabilityResponse {
                product_id: p.id,
                available: p.stock > 0,
                stock: p.stock,
            })
    }
}
