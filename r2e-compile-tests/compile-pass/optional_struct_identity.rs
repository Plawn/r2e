//! Phase 4: optional struct-level identity. `#[inject(identity)] user:
//! Option<AuthenticatedUser>` makes every endpoint work authenticated or
//! anonymous, with `self.user` available as `Option<_>` on the façade.

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

#[controller(path = "/opt", state = AppState)]
pub struct OptionalIdentityController {
    #[inject(identity)]
    user: Option<AuthenticatedUser>,
}

#[routes]
impl OptionalIdentityController {
    #[get("/whoami")]
    async fn whoami(&self) -> Json<String> {
        match &self.user {
            Some(u) => Json(u.sub().to_string()),
            None => Json("anonymous".to_string()),
        }
    }
}

fn main() {}
