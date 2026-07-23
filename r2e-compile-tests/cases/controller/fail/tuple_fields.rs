use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller(path = "/test")]
pub struct MyController(String, i32);

fn main() {}
