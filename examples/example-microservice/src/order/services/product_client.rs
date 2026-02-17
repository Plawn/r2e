use r2e::prelude::*;

use crate::shared::types::{AvailabilityResponse, ProductInfo};

/// HTTP client wrapper for the Product Service.
/// Injected as a bean with the product service URL from configuration.
#[derive(Clone)]
pub struct ProductClient {
    client: reqwest::Client,
    base_url: String,
}

#[bean]
impl ProductClient {
    pub fn new(#[config("services.product.url")] base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    pub async fn get_product(&self, id: u64) -> Result<ProductInfo, AppError> {
        let url = format!("{}/products/{}", self.base_url, id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Product service unavailable: {}", e)))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::NotFound(format!("Product {} not found", id)));
        }

        if !resp.status().is_success() {
            return Err(AppError::Internal(format!(
                "Product service returned {}",
                resp.status()
            )));
        }

        resp.json::<ProductInfo>()
            .await
            .map_err(|e| AppError::Internal(format!("Invalid product response: {}", e)))
    }

    pub async fn check_availability(&self, id: u64) -> Result<AvailabilityResponse, AppError> {
        let url = format!("{}/products/{}/availability", self.base_url, id);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("Product service unavailable: {}", e)))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::NotFound(format!("Product {} not found", id)));
        }

        if !resp.status().is_success() {
            return Err(AppError::Internal(format!(
                "Product service returned {}",
                resp.status()
            )));
        }

        resp.json::<AvailabilityResponse>()
            .await
            .map_err(|e| AppError::Internal(format!("Invalid availability response: {}", e)))
    }
}
