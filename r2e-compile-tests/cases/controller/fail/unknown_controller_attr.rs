use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller(path = "/test", state = AppState, foo = "bar")]
pub struct MyController;

fn main() {}
