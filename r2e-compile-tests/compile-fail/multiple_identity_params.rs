use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub claims_validator: Arc<r2e::r2e_security::JwtClaimsValidator>,
}

#[derive(Controller)]
#[controller(path = "/test", state = AppState)]
pub struct MyController;

#[routes]
impl MyController {
    #[get("/")]
    async fn handler(
        &self,
        #[inject(identity)] user1: AuthenticatedUser,
        #[inject(identity)] user2: AuthenticatedUser,
    ) -> &'static str {
        "test"
    }
}

fn main() {}
