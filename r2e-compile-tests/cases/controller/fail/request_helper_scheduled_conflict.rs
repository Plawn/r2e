//! `#[request_helper]` (façade) and `#[scheduled]` (core) are mutually exclusive
//! execution scopes — combining them on one method is rejected. The
//! request-helper arm is classified first, so it names the conflict.

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
    #[scheduled(every = 60)]
    async fn who(&self) {
        let _ = self.user.sub();
    }
}

fn main() {}
