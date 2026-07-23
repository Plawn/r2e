//! An `#[anonymous]` route runs on the controller **core**, where a
//! `#[request_helper]` (emitted on the façade) does not exist — calling it there
//! is a "method not found" compile error. This is the intended scope boundary:
//! request helpers are reachable only from request-scoped methods (routes/SSE/WS).

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use r2e::r2e_security::JwtClaimsValidator;
use r2e::Identity;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub claims_validator: Arc<JwtClaimsValidator>,
}

impl FromRef<AppState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &AppState) -> Self {
        state.claims_validator.clone()
    }
}

#[controller(path = "/rh")]
pub struct MyController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl MyController {
    #[request_helper]
    fn subject(&self) -> String {
        self.user.sub().to_string()
    }

    // Anonymous route runs on the core: `subject` (a façade method) is not in
    // scope here.
    #[get("/public")]
    #[anonymous]
    async fn public(&self) -> Json<String> {
        Json(self.subject())
    }
}

fn main() {}
