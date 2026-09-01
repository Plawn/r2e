//! Scaffolding for `llm/configuration.md`.

use r2e::prelude::*;

pub use std::path::PathBuf;
pub use std::sync::Arc;

/// Not in the prelude: `use r2e::{BeanContext, ServiceComponent};` and
/// `use r2e::rt;`.
pub use r2e::rt;
pub use r2e::{BeanContext, ServiceComponent};

/// The section the `load_config` snippet lists but does not spell out.
#[derive(ConfigProperties, Clone, Debug)]
pub struct AppConfig {
    #[config(default = "hello")]
    pub greeting: String,
}

/// A `DatabaseConfig` for the fixture-level `RootConfig` (the doc block
/// declares its own copy of both).
#[derive(ConfigProperties, Clone, Debug)]
pub struct DatabaseConfig {
    pub url: String,
    #[config(default = 5)]
    pub pool_size: i64,
}

/// The root config type `load_config::<RootConfig>()` names.
#[derive(ConfigProperties, Clone, Debug)]
pub struct RootConfig {
    #[config(section)]
    pub app: AppConfig,
    #[config(section)]
    pub database: DatabaseConfig,
}

/// The already-in-hand typed config of the `provide_config` snippet.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct DatabaseSettings {
    #[config(default = ":memory:")]
    pub url: String,
}

#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct AppSettings {
    #[config(section)]
    pub db: DatabaseSettings,
}

/// The controller that injects the nested section `provide_config` registers.
#[controller(path = "/users")]
pub struct UserController {
    #[inject]
    db: DatabaseSettings,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> String {
        self.db.url.clone()
    }
}

/// An external config source, as `.with_config_provider(..)` expects.
pub struct VaultConfigProvider {
    _address: String,
}

impl VaultConfigProvider {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            _address: address.into(),
        }
    }
}

impl ConfigProvider for VaultConfigProvider {
    fn load(
        &self,
        _config: &mut R2eConfig,
        _ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError> {
        Ok(())
    }
}

/// The worker built by the `#[producer(start)]` live-config snippet: produced
/// as a bean *and* started as a service, so it implements `ServiceComponent`.
#[derive(Clone)]
pub struct SearchClient {
    _endpoint: LiveConfig<String>,
}

impl SearchClient {
    pub async fn connect(endpoint: LiveConfig<String>) -> Result<Self, ConfigError> {
        Ok(Self {
            _endpoint: endpoint,
        })
    }
}

impl ServiceComponent for SearchClient {
    type Deps = r2e::type_list::TCons<SearchClient, r2e::type_list::TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<SearchClient>()
    }

    async fn start(self, shutdown: rt::CancelToken) {
        shutdown.cancelled().await;
    }
}
