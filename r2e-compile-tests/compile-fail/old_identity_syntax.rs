use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller(state = AppState)]
pub struct OldIdentityController {
    #[identity]
    user: String,
}

fn main() {}
