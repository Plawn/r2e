use r2e::prelude::*;

#[controller(path = "/test")]
pub struct MyController {
    #[inject]
    name: String,
}

fn main() {}
