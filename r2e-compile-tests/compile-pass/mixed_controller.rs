use r2e::prelude::*;
use r2e::Identity;
use r2e::r2e_security::AuthenticatedUser;
use r2e::r2e_security::JwtClaimsValidator;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub greeting: String,
    pub claims_validator: Arc<JwtClaimsValidator>,
}

impl FromRef<AppState> for String {
    fn from_ref(state: &AppState) -> Self {
        state.greeting.clone()
    }
}

impl FromRef<AppState> for Arc<JwtClaimsValidator> {
    fn from_ref(state: &AppState) -> Self {
        state.claims_validator.clone()
    }
}

#[derive(Controller)]
#[controller(path = "/mixed", state = AppState)]
pub struct MixedController {
    #[inject]
    greeting: String,
}

#[routes]
impl MixedController {
    #[get("/public")]
    async fn public_data(&self) -> String {
        self.greeting.clone()
    }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<String> {
        Json(user.sub().to_string())
    }
}

fn main() {}
