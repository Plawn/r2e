use r2e::prelude::*;

#[derive(Clone)]
pub struct MyService;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub service: MyService,
    pub config: R2eConfig,
}

fn main() {}
