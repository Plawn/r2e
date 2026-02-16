use r2e::prelude::*;

#[derive(Controller)]
pub struct MyController {
    #[inject]
    name: String,
}

fn main() {}
