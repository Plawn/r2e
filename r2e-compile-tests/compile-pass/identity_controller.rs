use r2e::prelude::*;
use r2e::Identity;
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

#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct IdentityController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl IdentityController {
    #[get("/me")]
    async fn me(&self) -> Json<String> {
        Json(self.user.sub().to_string())
    }
}

fn main() {}
