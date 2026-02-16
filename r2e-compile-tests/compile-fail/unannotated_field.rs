use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState {
    pub name: String,
}

#[derive(Controller)]
#[controller(path = "/test", state = AppState)]
pub struct MyController {
    name: String,
}

fn main() {}
