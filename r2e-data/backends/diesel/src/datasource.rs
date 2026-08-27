//! The datasource plugin: `datasource.*` config in, a connected
//! [`DbPool`](crate::DbPool) bean out — plus optional migrations at boot.
//!
//! ```ignore
//! const MIGRATIONS: EmbeddedMigrations = embed_migrations!("./migrations");
//!
//! AppBuilder::new()
//!     .load_config::<()>()
//!     .plugin(DieselDataSource::<PgConnection>::new().migrations(&MIGRATIONS))
//!     .register_controller::<ArticleController>()
//! ```
//!
//! ```yaml
//! datasource:
//!   url: "postgres://user:pass@localhost/app"
//!   max-connections: 20
//!   min-connections: 2
//!   acquire-timeout: 10s
//!   migrate-at-start: true
//! ```
//!
//! # Named datasources
//!
//! A second database is a second *tag*: [`datasource_tag!`] mints a zero-sized
//! marker whose config lives under `datasource.<name>.*` and whose bean type is
//! `DbPool<Conn, Tag>` — distinct from `DbPool<Conn>`, so both can be installed
//! in one app and injected without ambiguity.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use diesel::backend::Backend;
use diesel::migration::{Migration, MigrationSource};
use diesel::r2d2::{ConnectionManager, Pool, R2D2Connection};
use diesel::Connection;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness};
use r2e_core::config::LiveConfig;
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::prelude::ConfigProperties;
use r2e_core::LiveConfigRegistry;

use crate::pool::{PoolError, PoolFactory};
use crate::DbPool;

/// Compile-time name of a datasource, and with it the config section and the
/// bean type of its pool.
///
/// Implement it through [`datasource_tag!`] rather than by hand — the macro
/// keeps [`NAME`](Self::NAME) and [`CONFIG_PREFIX`](Self::CONFIG_PREFIX) in
/// step, which nothing else can (a `const` cannot concatenate strings).
pub trait DataSourceTag: Send + Sync + 'static {
    /// The datasource's name, or `None` for the app's default (unnamed) one.
    const NAME: Option<&'static str>;

    /// The YAML section the datasource reads: `datasource` for the default
    /// tag, `datasource.<name>` for a named one.
    const CONFIG_PREFIX: &'static str;
}

/// The app's single, unnamed datasource — config section `datasource`.
///
/// It is the default `Tag` of [`DbPool`], so `DbPool<PgConnection>` is the
/// default datasource's pool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DefaultDataSource;

impl DataSourceTag for DefaultDataSource {
    const NAME: Option<&'static str> = None;
    const CONFIG_PREFIX: &'static str = "datasource";
}

/// Declare a named datasource tag: a zero-sized marker type implementing
/// [`DataSourceTag`], reading `datasource.<name>.*`.
///
/// ```ignore
/// r2e_data_diesel::datasource_tag!(pub Reporting = "reporting");
///
/// // config section `datasource.reporting`, bean `DbPool<PgConnection, Reporting>`
/// b.plugin(DieselDataSource::<PgConnection, Reporting>::new())
/// ```
#[macro_export]
macro_rules! datasource_tag {
    ($(#[$meta:meta])* $vis:vis $name:ident = $key:literal) => {
        $(#[$meta])*
        #[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::marker::Copy, Default)]
        $vis struct $name;

        impl $crate::DataSourceTag for $name {
            const NAME: ::core::option::Option<&'static str> = ::core::option::Option::Some($key);
            const CONFIG_PREFIX: &'static str = ::core::concat!("datasource.", $key);
        }
    };
}

/// The `datasource.*` (or `datasource.<name>.*`) section.
///
/// Every field is optional except through its default: an app that only sets
/// `url` gets r2d2's own pool defaults, and no migrations.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct DataSourceConfig {
    /// Connection URL. Read as a **live** value too: the pool rotates onto a
    /// new URL published for this key at runtime (see [`DbPool`]).
    pub url: Option<String>,

    /// Maximum size of the r2d2 pool (`max_size`). Default: r2d2's own (10).
    #[config(key = "max-connections")]
    pub max_connections: Option<u32>,

    /// Minimum number of idle connections r2d2 keeps (`min_idle`). Default:
    /// r2d2's own (equal to `max_size`).
    #[config(key = "min-connections")]
    pub min_connections: Option<u32>,

    /// How long `get()` waits for a free connection (`connection_timeout`).
    /// Accepts an integer (seconds) or a duration string like `"10s"`.
    #[config(key = "acquire-timeout")]
    pub acquire_timeout: Option<Duration>,

    /// Run the migrations handed to [`DieselDataSource::migrations`] during
    /// boot. Default: `false`.
    #[config(key = "migrate-at-start", default = false)]
    pub migrate_at_start: bool,
}

/// Plugin that owns a database's whole boot: connect, migrate, dispose.
///
/// Provides one bean — `DbPool<Conn, Tag>` — from the `datasource` section (or
/// `datasource.<name>` for a named [`Tag`](DataSourceTag)). Failure to build
/// the pool or to migrate aborts startup with
/// `Plugin 'DieselDataSource' failed to build`.
///
/// It is the Diesel mirror of `SqlxDataSource`: same config section, same
/// `migrate-at-start` gate, same live-URL rotation — with r2d2's blocking pool
/// build and blocking migration run pushed onto the blocking pool.
///
/// # `datasource.enabled`
///
/// There is **no** enabled gate: a pool bean has no inert form — the graph
/// promises a `DbPool` and every consumer would fail on it anyway. Setting
/// `<prefix>.enabled = false` only logs a warning and is otherwise ignored.
pub struct DieselDataSource<Conn, Tag = DefaultDataSource> {
    migrations: Option<&'static EmbeddedMigrations>,
    marker: PhantomData<fn() -> (Conn, Tag)>,
}

impl<Conn, Tag> Default for DieselDataSource<Conn, Tag> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Conn, Tag> DieselDataSource<Conn, Tag> {
    /// A datasource with no migrations: `migrate-at-start` has nothing to run.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            migrations: None,
            marker: PhantomData,
        }
    }

    /// Attach the compile-time migration set
    /// (`diesel_migrations::embed_migrations!("./migrations")`, stored in a
    /// `const`/`static` so the reference is `'static`).
    ///
    /// Attaching it does not run it: `datasource.migrate-at-start: true` does,
    /// so the same binary can migrate in dev and stay read-only in production.
    #[must_use]
    pub const fn migrations(mut self, migrations: &'static EmbeddedMigrations) -> Self {
        self.migrations = Some(migrations);
        self
    }
}

/// `MigrationSource` is implemented for `EmbeddedMigrations` by value, but the
/// plugin holds a `&'static` one (a `const`/`static` in the app). This forwards
/// the borrowed set as an owned source without cloning the migrations.
struct StaticMigrations(&'static EmbeddedMigrations);

impl<DB: Backend> MigrationSource<DB> for StaticMigrations {
    fn migrations(&self) -> diesel::migration::Result<Vec<Box<dyn Migration<DB>>>> {
        MigrationSource::<DB>::migrations(self.0)
    }
}

impl<Conn, Tag> Plugin for DieselDataSource<Conn, Tag>
where
    Conn: Connection + R2D2Connection + MigrationHarness<Conn::Backend> + Send + 'static,
    Tag: DataSourceTag,
{
    type Provided = (DbPool<Conn, Tag>,);
    /// The registry is what makes the URL *live*: the pool subscribes to
    /// `<prefix>.url` through it and rotates when a provider publishes a new
    /// value. It arrives with `load_config()`, so an app without config gets
    /// the ordinary missing-bean error at `build_state()`.
    type Deps = (LiveConfigRegistry,);
    type Config = DataSourceConfig;
    type Controllers = ();
    const CONFIG_PREFIX: Option<&'static str> = Some(Tag::CONFIG_PREFIX);

    /// `build` produces exactly one bean, and both effects it registers act on
    /// the pool `build` itself creates. A test that pins the pool has replaced
    /// all of that, so skipping the build (and the migrations) is right.
    const SKIP_BUILD_WHEN_ALL_PINNED: bool = true;

    async fn build(
        self,
        (live,): Self::Deps,
        config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        let prefix = Tag::CONFIG_PREFIX;
        if !ctx.enabled() {
            tracing::warn!(
                "`{prefix}.enabled = false` is ignored: a datasource has no inert form — \
                 the `DbPool` bean must exist for anything that injects it. \
                 Point `{prefix}.url` elsewhere, or pin the pool in a test."
            );
        }

        let config = config.unwrap_or_default();
        if config.url.is_none() {
            return Err(format!(
                "`{prefix}.url` is not set: the datasource has no database to connect to"
            )
            .into());
        }

        // r2d2's builder is consumed per build, and a rotation rebuilds the
        // pool — so the settings are captured in the factory, not applied once.
        let max_connections = config.max_connections;
        let min_connections = config.min_connections;
        let acquire_timeout = config.acquire_timeout;
        let factory: PoolFactory<Conn> = Arc::new(move |url: String| {
            let mut builder = Pool::builder();
            if let Some(max) = max_connections {
                builder = builder.max_size(max);
            }
            if let Some(min) = min_connections {
                builder = builder.min_idle(Some(min));
            }
            if let Some(timeout) = acquire_timeout {
                builder = builder.connection_timeout(timeout);
            }
            builder
                .build(ConnectionManager::<Conn>::new(url))
                .map_err(|error| PoolError(error.to_string()))
        });

        // The URL is read through the live registry, not copied out of
        // `config`: same key, but the handle keeps rotating with it.
        let url: LiveConfig<String> = live.live_config(format!("{prefix}.url"));
        let pool = DbPool::<Conn, Tag>::connect_with(factory, url).await?;

        match (config.migrate_at_start, self.migrations) {
            (true, Some(migrations)) => {
                let target = pool.current();
                // Diesel's harness is blocking, and so is taking an r2d2
                // connection: both belong on the blocking pool.
                r2e_core::rt::spawn_blocking(move || -> Result<(), PluginBuildError> {
                    let mut connection = target.get().map_err(|error| -> PluginBuildError {
                        format!("could not take a connection to migrate: {error}").into()
                    })?;
                    connection.run_pending_migrations(StaticMigrations(migrations))?;
                    Ok(())
                })
                .await
                .map_err(|error| -> PluginBuildError {
                    format!("migration task failed: {error}").into()
                })??;
                tracing::info!(datasource = prefix, "database migrations applied");
            }
            (true, None) => tracing::warn!(
                "`{prefix}.migrate-at-start` is true but no migration set was attached: \
                 call `.migrations(&MIGRATIONS)` on the plugin \
                 (`diesel_migrations::embed_migrations!(\"./migrations\")`)"
            ),
            (false, Some(_)) => tracing::debug!(
                datasource = prefix,
                "migrations attached but `{prefix}.migrate-at-start` is false — not running them"
            ),
            (false, None) => {}
        }

        // Rotation: `DbPool` is a `ServiceComponent`; nothing else would start
        // its watch loop now that the pool is not a `#[producer(start)]`. The
        // token comes from the framework, so the loop stops on every exit.
        let watcher = pool.clone();
        ctx.on_serve(move |serve| {
            let shutdown = serve.shutdown_token();
            serve.track(async move {
                r2e_core::ServiceComponent::start(watcher, shutdown).await;
            });
        });

        // Unlike SQLx, r2d2 pools have no async close: dropping the last handle
        // closes the idle connections, and in-flight ones close on return. The
        // framework drops the bean graph at shutdown, so there is nothing to
        // register here.

        Ok((pool,))
    }
}
