use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[derive(Controller)]
#[controller(path = "/test", state = AppState, foo = "bar")]
pub struct MyController;

fn main() {}
