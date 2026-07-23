//! This is *why* you want `#[request_helper]`.
//!
//! A PLAIN (unmarked) helper stays on the controller core, where the
//! request-scoped identity field does not exist — so `self.user` fails with a
//! bare "no field `user`" error. Mark the helper `#[request_helper]` to move it
//! onto the façade (where `self.user` lives), or pass the identity in as a
//! parameter.

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
    #[get("/who")]
    async fn who(&self) -> Json<String> {
        Json(self.subject())
    }

    // Plain helper on the core — `self.user` does not exist here.
    fn subject(&self) -> String {
        self.user.sub().to_string()
    }
}

fn main() {}
