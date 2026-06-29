use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller(path = "/test", state = AppState)]
pub enum MyController {
    Variant1,
    Variant2,
}

fn main() {}
