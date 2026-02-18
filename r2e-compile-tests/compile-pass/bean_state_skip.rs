use r2e::prelude::*;

#[derive(Clone)]
pub struct MyService;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub service: MyService,
    #[bean_state(skip_from_ref)]
    pub internal: String,
}

fn main() {}
