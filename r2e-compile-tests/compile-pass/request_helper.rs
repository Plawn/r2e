//! `#[request_helper]`: a helper method moved onto the per-request façade so it
//! can read the request-scoped identity / `#[inject(request)]` fields directly,
//! and reach `#[inject]`/`#[config]` fields and core helpers through `Deref`.
//!
//! A request helper is callable only from other façade methods (routes/SSE/WS);
//! it coexists in the same impl with off-request methods (`#[consumer]` /
//! `#[scheduled]`) that run on the core and call *core* helpers.

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

impl FromRef<AppState> for LocalEventBus {
    fn from_ref(state: &AppState) -> Self {
        state.event_bus.clone()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserCreated {
    pub name: String,
}

#[controller(path = "/acct")]
pub struct AccountController {
    #[inject]
    event_bus: LocalEventBus,
    #[inject]
    prefix: String,
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl AccountController {
    // Request helper — lives on the façade. Reads the request-scoped identity
    // and reaches the core `prefix` field + a core helper through `Deref`.
    #[request_helper]
    fn labelled_user(&self) -> String {
        format!("{}{}", self.decorate(), self.user.sub())
    }

    // Async request helper.
    #[request_helper]
    async fn who_async(&self) -> String {
        self.user.sub().to_string()
    }

    // Route: runs on the façade, calls both request helpers.
    #[get("/me")]
    async fn me(&self) -> Json<String> {
        let _ = self.who_async().await;
        Json(self.labelled_user())
    }

    // Core helper: reached from the façade route via `Deref`, and also from the
    // off-request methods below (which run on the core).
    fn decorate(&self) -> String {
        format!("[{}]", self.prefix)
    }

    // Off-request consumer: runs on the core, calls the core helper — never the
    // request helper.
    #[consumer(bus = "event_bus")]
    async fn on_user_created(&self, event: Arc<UserCreated>) {
        let _ = (&event.name, self.decorate());
    }

    // Off-request scheduled: runs on the core, core fields / core helpers only.
    #[scheduled(every = 60)]
    async fn heartbeat(&self) {
        let _ = self.decorate();
    }
}

// A controller whose only methods are request helpers (no routes). The façade
// type is always generated, so the helpers compile onto it even with no routes.
#[controller(path = "/only")]
pub struct HelperOnlyController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl HelperOnlyController {
    #[request_helper]
    fn subject(&self) -> String {
        self.user.sub().to_string()
    }
}

fn main() {}
