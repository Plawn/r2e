use r2e::prelude::*;
use r2e::r2e_data_sqlx::HasPool;
use sqlx::{Pool, Postgres};

use crate::services::ArticleService;

#[derive(Clone, BeanState)]
pub struct AppState {
    pub article_service: ArticleService,
    pub pool: sqlx::PgPool,
    pub config: R2eConfig,
}

impl HasPool<Postgres> for AppState {
    fn pool(&self) -> &Pool<Postgres> {
        &self.pool
    }
}
