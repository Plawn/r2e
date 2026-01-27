use crate::models::User;
use crate::services::UserService;
use crate::state::Services;
use quarlus_security::AuthenticatedUser;

#[derive(quarlus_macros::Controller)]
#[controller(state = Services)]
pub struct AccountController {
    #[inject]
    user_service: UserService,

    #[identity]
    user: AuthenticatedUser,

    #[config("app.greeting")]
    greeting: String,
}

#[quarlus_macros::routes]
impl AccountController {
    #[get("/greeting")]
    async fn greeting(&self) -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({ "greeting": self.greeting }))
    }

    #[get("/error/custom")]
    async fn custom_error(&self) -> Result<axum::Json<()>, quarlus_core::AppError> {
        Err(quarlus_core::AppError::Custom {
            status: axum::http::StatusCode::from_u16(418).unwrap(),
            body: serde_json::json!({ "error": "I'm a teapot", "code": 418 }),
        })
    }

    #[get("/me")]
    async fn me(&self) -> axum::Json<AuthenticatedUser> {
        axum::Json(self.user.clone())
    }

    #[get("/admin/users")]
    #[roles("admin")]
    async fn admin_list(&self) -> axum::Json<Vec<User>> {
        let users = self.user_service.list().await;
        axum::Json(users)
    }
}
