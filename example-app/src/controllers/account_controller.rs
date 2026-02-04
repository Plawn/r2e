use crate::models::User;
use crate::services::UserService;
use crate::state::Services;
use quarlus::prelude::*;
use quarlus::quarlus_security::AuthenticatedUser;

#[derive(Controller)]
#[controller(state = Services)]
pub struct AccountController {
    #[inject]
    user_service: UserService,

    #[inject(identity)]
    user: AuthenticatedUser,

    #[config("app.greeting")]
    greeting: String,
}

#[routes]
impl AccountController {
    #[get("/greeting")]
    async fn greeting(&self) -> Json<serde_json::Value> {
        Json(serde_json::json!({ "greeting": self.greeting }))
    }

    #[get("/error/custom")]
    async fn custom_error(&self) -> Result<Json<()>, AppError> {
        Err(AppError::Custom {
            status: StatusCode::from_u16(418).unwrap(),
            body: serde_json::json!({ "error": "I'm a teapot", "code": 418 }),
        })
    }

    #[get("/me")]
    async fn me(&self) -> Json<AuthenticatedUser> {
        Json(self.user.clone())
    }

    #[get("/admin/users")]
    #[roles("admin")]
    async fn admin_list(&self) -> Json<Vec<User>> {
        let users = self.user_service.list().await;
        Json(users)
    }
}
