use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[derive(Controller)]
#[controller(path = "/test", state = AppState)]
pub struct MyController(String, i32);

fn main() {}
