use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller(path = "/test")]
pub enum MyController {
    Variant1,
    Variant2,
}

fn main() {}
