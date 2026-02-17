use std::sync::Arc;

use r2e::prelude::*;
use r2e::r2e_security::JwtClaimsValidator;

use crate::services::ProjectService;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub project_service: ProjectService,
    pub claims_validator: Arc<JwtClaimsValidator>,
    pub pool: sqlx::SqlitePool,
    pub config: R2eConfig,
}
