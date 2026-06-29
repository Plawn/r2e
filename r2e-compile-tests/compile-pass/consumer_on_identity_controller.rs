//! Phase 4: a controller may declare struct-level identity AND off-request
//! methods (`#[consumer]` / `#[scheduled]`) as long as those off-request methods
//! use only core fields. Route methods run on the façade (where `self.user`
//! exists); consumers/scheduled run on the core (rebuilt from state) and never
//! see request identity.

use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;
use r2e::r2e_security::AuthenticatedUser;
use r2e::r2e_security::JwtClaimsValidator;
use r2e::Identity;
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
    // Route method: runs on the façade, may read `self.user`.
    #[get("/me")]
    async fn me(&self) -> Json<String> {
        Json(self.user.sub().to_string())
    }

    // Consumer: runs on the core. Uses only the core `event_bus` field — never
    // `self.user`.
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreated>) {
        let _ = &event.name;
        let _ = &self.event_bus;
    }

    // Scheduled: runs on the core, core fields only.
    #[scheduled(every = 60)]
    async fn heartbeat(&self) {
        let _ = &self.event_bus;
    }
}

fn main() {}
