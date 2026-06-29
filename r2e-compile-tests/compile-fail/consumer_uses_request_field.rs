//! Phase 4 diagnostic: a `#[consumer]` (off-request) method cannot touch a
//! request-scoped field. Consumers run on the controller core, which no longer
//! holds request-scoped fields — so `self.user` does not exist there.

use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;
use r2e::r2e_security::AuthenticatedUser;
use r2e::r2e_security::JwtClaimsValidator;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub event_bus: LocalEventBus,
    pub claims_validator: Arc<JwtClaimsValidator>,
}

impl FromRef<AppState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &AppState) -> Self {
        state.claims_validator.clone()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCreated {
    pub name: String,
}

#[controller(path = "/acct", state = AppState)]
pub struct AccountController {
    #[inject]
    event_bus: LocalEventBus,
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl AccountController {
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreated>) {
        // ERROR: `user` is a request-scoped field; it lives on the façade, not
        // the core where this consumer runs.
        let _ = (&event.name, &self.user);
    }
}

fn main() {}
