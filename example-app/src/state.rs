use std::sync::Arc;

use axum::extract::FromRef;
use quarlus_core::QuarlusConfig;
use quarlus_events::EventBus;
use quarlus_rate_limit::RateLimitRegistry;
use quarlus_security::JwtValidator;
use tokio_util::sync::CancellationToken;

use crate::services::UserService;

#[derive(Clone)]
pub struct Services {
    pub user_service: UserService,
    pub jwt_validator: Arc<JwtValidator>,
    pub pool: sqlx::SqlitePool,
    pub event_bus: EventBus,
    pub config: QuarlusConfig,
    pub cancel: CancellationToken,
    pub rate_limiter: RateLimitRegistry,
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

impl FromRef<Services> for QuarlusConfig {
    fn from_ref(state: &Services) -> Self {
        state.config.clone()
    }
}

impl FromRef<Services> for EventBus {
    fn from_ref(state: &Services) -> Self {
        state.event_bus.clone()
    }
}

impl FromRef<Services> for RateLimitRegistry {
    fn from_ref(state: &Services) -> Self {
        state.rate_limiter.clone()
    }
}
