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
#[controller(path = "/api", state = AppState)]
pub struct OptionalIdentityController {
    #[inject]
    greeting: String,
}

#[routes]
impl OptionalIdentityController {
    #[get("/whoami")]
    async fn whoami(
        &self,
        #[inject(identity)] user: Option<AuthenticatedUser>,
    ) -> Json<String> {
        match user {
            Some(u) => Json(format!("Hello, {}", u.sub())),
            None => Json("Hello, anonymous".to_string()),
        }
    }
}

fn main() {}
