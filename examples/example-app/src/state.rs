use std::sync::Arc;

use r2e::prelude::*;
use r2e::r2e_data_sqlx::HasPool;
use r2e::r2e_rate_limit::RateLimitRegistry;
use r2e::r2e_security::JwtClaimsValidator;
use sqlx::{Pool, Sqlite};
use tokio_util::sync::CancellationToken;

use crate::services::{NotificationService, UserService};

#[derive(Clone, BeanState)]
pub struct Services {
    pub user_service: UserService,
    pub claims_validator: Arc<JwtClaimsValidator>,
    pub pool: sqlx::SqlitePool,
    pub event_bus: EventBus,
    pub config: R2eConfig,
    pub cancel: CancellationToken,
    pub rate_limiter: RateLimitRegistry,
    pub sse_broadcaster: r2e::sse::SseBroadcaster,
    pub notification_service: NotificationService,
}

impl HasPool<Sqlite> for Services {
    fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }
}
