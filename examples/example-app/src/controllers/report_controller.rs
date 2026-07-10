use crate::models::User;
use crate::services::UserService;
use r2e::prelude::*;

/// Fail-closed authentication with `#[anonymous]` opt-out.
///
/// The struct-level identity makes **every** route require a valid JWT by
/// default; the public exceptions are marked `#[anonymous]` (@PermitAll).
/// Anonymous routes run on the controller core: identity extraction is
/// skipped entirely and reading `self.user` there is a compile error.
#[controller(path = "/reports")]
pub struct ReportController {
    #[inject]
    user_service: UserService,
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl ReportController {
    /// Public — anyone can see the report summary. No JWT validation runs.
    #[get("/summary")]
    #[anonymous]
    async fn summary(&self) -> Json<usize> {
        Json(self.user_service.list().await.len())
    }

    /// Authenticated by default — no marker needed, and the identity is
    /// available as a plain field (no `Option`).
    #[get("/full")]
    async fn full(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }

    /// Role-gated — `#[roles]` reads the struct identity directly.
    #[get("/audit")]
    #[roles("admin")]
    async fn audit(&self) -> Json<String> {
        Json(format!("audit requested by {}", self.user.sub))
    }
}
