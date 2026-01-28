use crate::models::User;
use crate::services::UserService;
use crate::state::Services;
use quarlus_core::prelude::*;
use quarlus_security::AuthenticatedUser;

/// A mixed controller demonstrating param-level identity injection.
///
/// Because there is no `#[inject(identity)]` on the struct, `StatefulConstruct`
/// is generated — enabling this controller for consumers and scheduled tasks
/// while still having protected endpoints via handler-param identity.
#[derive(Controller)]
#[controller(path = "/mixed", state = Services)]
pub struct MixedController {
    #[inject]
    user_service: UserService,
}

#[routes]
impl MixedController {
    /// Public endpoint — no authentication required.
    #[get("/public")]
    async fn public_data(&self) -> axum::Json<Vec<User>> {
        let users = self.user_service.list().await;
        axum::Json(users)
    }

    /// Protected endpoint — identity injected as handler parameter.
    #[get("/me")]
    async fn me(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> axum::Json<AuthenticatedUser> {
        axum::Json(user)
    }

    /// Protected endpoint with role check — identity from handler parameter.
    #[get("/admin")]
    #[roles("admin")]
    async fn admin_only(
        &self,
        #[inject(identity)] _user: AuthenticatedUser,
    ) -> axum::Json<Vec<User>> {
        let users = self.user_service.list().await;
        axum::Json(users)
    }
}
