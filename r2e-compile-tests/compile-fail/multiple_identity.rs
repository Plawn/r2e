use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;

#[derive(Clone)]
pub struct AppState;

#[derive(Controller)]
#[controller(path = "/test", state = AppState)]
pub struct MyController {
    #[inject(identity)]
    user1: AuthenticatedUser,
    #[inject(identity)]
    user2: AuthenticatedUser,
}

fn main() {}
