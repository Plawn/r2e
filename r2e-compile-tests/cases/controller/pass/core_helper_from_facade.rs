//! Phase 4: a façade route method may call a core helper method through `Deref`,
//! as long as the helper does not use a request-scoped field. The helper stays
//! on `impl <Name>` (the core); the route lives on the façade.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use r2e::r2e_security::JwtClaimsValidator;
use r2e::Identity;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub claims_validator: Arc<JwtClaimsValidator>,
    pub prefix: String,
}

impl FromRef<AppState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &AppState) -> Self {
        state.claims_validator.clone()
    }
}

impl FromRef<AppState> for String {
    fn from_ref(state: &AppState) -> Self {
        state.prefix.clone()
    }
}

#[controller(path = "/helper")]
pub struct HelperController {
    #[inject]
    prefix: String,
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl HelperController {
    #[get("/greet")]
    async fn greet(&self) -> String {
        // Façade route: reads `self.user` (façade field) and calls a core helper
        // through `Deref`.
        format!("{} {}", self.decorate(), self.user.sub())
    }

    // A non-route helper inside `#[routes]` — emitted on the core `impl`. Uses
    // only core fields and is reached from the façade route via `Deref`.
    fn decorate(&self) -> String {
        format!("[{}]", self.prefix)
    }
}

fn main() {}
