use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct OldIdentityController {
    #[identity]
    user: String,
}

fn main() {}
