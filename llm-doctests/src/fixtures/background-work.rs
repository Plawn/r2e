//! Scaffolding for `llm/background-work.md`.

use r2e::prelude::*;

/// The runtime façade the service signatures name (`use r2e::rt;`).
pub use r2e::rt;

/// Not in the prelude: `use r2e::BeanContext;` / `use r2e::ServiceComponent;`
/// / `use r2e::type_list::{TCons, TNil};`.
pub use r2e::type_list::{TCons, TNil};
pub use r2e::{BeanContext, ServiceComponent};

/// The Executor plugin and the pool bean it provides
/// (`use r2e::r2e_executor::{Executor, PoolExecutor};`).
pub use r2e::r2e_executor::{Executor, PoolExecutor};

pub use sqlx::SqlitePool;

/// The app's root config type passed to `load_config::<RootConfig>()`.
#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    pub greeting: Option<String>,
}

/// Registers the `SqlitePool` the snippets inject.
#[producer]
pub async fn create_pool(#[config("database.url")] url: String) -> Result<SqlitePool, sqlx::Error> {
    SqlitePool::connect(&url).await
}

/// The heavy work the `#[async_exec]` method submits to the pool.
pub async fn render_pdf(_id: u64) -> Vec<u8> {
    Vec::new()
}

/// The collaborator the `#[producer(start)]` worker takes.
#[derive(Clone)]
pub struct Sink;

#[bean]
impl Sink {
    pub fn new() -> Self {
        Self
    }
}
