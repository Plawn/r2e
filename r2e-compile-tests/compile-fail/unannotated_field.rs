use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState {
    pub name: String,
}

#[controller(path = "/test", state = AppState)]
pub struct MyController {
    name: String,
}

fn main() {}
