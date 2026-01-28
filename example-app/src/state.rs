use std::sync::Arc;

use quarlus_core::prelude::*;
use quarlus_core::QuarlusConfig;
use quarlus_events::EventBus;
use quarlus_rate_limit::RateLimitRegistry;
use quarlus_security::JwtValidator;
use tokio_util::sync::CancellationToken;

use crate::services::UserService;

#[derive(Clone, BeanState)]
pub struct Services {
    pub user_service: UserService,
    pub jwt_validator: Arc<JwtValidator>,
    pub pool: sqlx::SqlitePool,
    pub event_bus: EventBus,
    pub config: QuarlusConfig,
    pub cancel: CancellationToken,
    pub rate_limiter: RateLimitRegistry,
}
