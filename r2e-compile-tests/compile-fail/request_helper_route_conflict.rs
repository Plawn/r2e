//! `#[request_helper]` cannot double as a route: it is a plain helper emitted on
//! the façade, not a route handler. Combining it with `#[get]` is rejected.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use r2e::r2e_security::JwtClaimsValidator;
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
    #[get("/who")]
    async fn who(&self) -> Json<String> {
        Json(self.user.sub().to_string())
    }
}

fn main() {}
