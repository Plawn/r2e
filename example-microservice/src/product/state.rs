use r2e::prelude::*;

use crate::services::ProductService;

#[derive(Clone, BeanState)]
pub struct ProductState {
    pub product_service: ProductService,
    pub config: R2eConfig,
}
