use r2e::prelude::*;

use crate::models::{AvailabilityResponse, ProductInfo};
use crate::services::ProductService;
use crate::state::ProductState;

#[derive(Controller)]
#[controller(path = "/products", state = ProductState)]
pub struct ProductController {
    #[inject]
    product_service: ProductService,
}

#[routes]
impl ProductController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<ProductInfo>> {
        Json(self.product_service.list().await)
    }

    #[get("/{id}")]
    async fn get_by_id(&self, Path(id): Path<u64>) -> Result<Json<ProductInfo>, HttpError> {
        self.product_service
            .get_by_id(id)
            .await
            .map(Json)
            .ok_or_else(|| HttpError::NotFound(format!("Product {} not found", id)))
    }

    #[get("/{id}/availability")]
    async fn check_availability(
        &self,
        Path(id): Path<u64>,
    ) -> Result<Json<AvailabilityResponse>, HttpError> {
        self.product_service
            .check_availability(id)
            .await
            .map(Json)
            .ok_or_else(|| HttpError::NotFound(format!("Product {} not found", id)))
    }
}
