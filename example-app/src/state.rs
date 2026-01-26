use std::sync::Arc;

use axum::extract::FromRef;
use quarlus_security::JwtValidator;

use crate::services::UserService;

#[derive(Clone)]
pub struct Services {
    pub user_service: UserService,
    pub jwt_validator: Arc<JwtValidator>,
    pub pool: sqlx::SqlitePool,
}

impl FromRef<Services> for Arc<JwtValidator> {
    fn from_ref(state: &Services) -> Self {
        state.jwt_validator.clone()
    }
}

impl FromRef<Services> for sqlx::SqlitePool {
    fn from_ref(state: &Services) -> Self {
        state.pool.clone()
    }
}
