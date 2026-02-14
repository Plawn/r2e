use r2e::prelude::*;

use crate::services::{OrderService, ProductClient};

#[derive(Clone, BeanState)]
pub struct OrderState {
    pub order_service: OrderService,
    pub product_client: ProductClient,
    pub config: R2eConfig,
}
