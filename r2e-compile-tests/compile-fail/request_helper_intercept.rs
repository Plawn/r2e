//! `#[intercept]` on a `#[request_helper]` is rejected — a plain helper has no
//! dispatch wrapper to run the interceptor chain (same rule as any plain method).

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
    #[intercept(Logged::info())]
    fn who(&self) -> String {
        self.user.sub().to_string()
    }
}

fn main() {}
