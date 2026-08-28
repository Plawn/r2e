//! The datasource plugin: `datasource.*` config in, a connected
//! [`DbPool`](crate::DbPool) bean out — plus optional migrations at boot.
//!
//! ```ignore
//! static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
//!
//! AppBuilder::new()
//!     .load_config::<()>()
//!     .plugin(SqlxDataSource::<sqlx::Postgres>::new().migrations(&MIGRATOR))
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
//! `DbPool<DB, Tag>` — distinct from `DbPool<DB>`, so both can be installed in
//! one app and injected without ambiguity.

use std::marker::PhantomData;
use std::time::Duration;

use r2e_core::config::LiveConfig;
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::prelude::ConfigProperties;
use r2e_core::LiveConfigRegistry;
use sqlx::migrate::{Migrate, Migrator};
use sqlx::pool::PoolOptions;
use sqlx::{Database, Executor};

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
/// It is the default `Tag` of [`DbPool`], so `DbPool<Postgres>` is the default
/// datasource's pool.
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
/// r2e_data_sqlx::datasource_tag!(pub Reporting = "reporting");
///
/// // config section `datasource.reporting`, bean `DbPool<Postgres, Reporting>`
/// b.plugin(SqlxDataSource::<Postgres, Reporting>::new())
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
/// `url` gets SQLx's own pool defaults, and no migrations.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct DataSourceConfig {
    /// Connection URL. Read as a **live** value too: the pool rotates onto a
    /// new URL published for this key at runtime (see [`DbPool`]).
    pub url: Option<String>,

    /// Maximum size of the connection pool. Default: SQLx's own (10).
    #[config(key = "max-connections")]
    pub max_connections: Option<u32>,

    /// Minimum number of idle connections the pool maintains. Default: SQLx's
    /// own (0).
    #[config(key = "min-connections")]
    pub min_connections: Option<u32>,

    /// How long `acquire()` waits for a free connection. Accepts an integer
    /// (seconds) or a duration string like `"10s"`, `"1m"`.
    #[config(key = "acquire-timeout")]
    pub acquire_timeout: Option<Duration>,

    /// Run the migrator handed to
    /// [`SqlxDataSource::migrations`] during boot. Default: `false`.
    #[config(key = "migrate-at-start", default = false)]
    pub migrate_at_start: bool,
}

/// Plugin that owns a database's whole boot: connect, migrate, close.
///
/// Provides one bean — `DbPool<DB, Tag>` — from the `datasource` section (or
/// `datasource.<name>` for a named [`Tag`](DataSourceTag)). Failure to connect
/// or to migrate aborts startup with `Plugin 'SqlxDataSource' failed to build`.
///
/// It replaces the `#[producer(start)] fn create_pool(...)` +
/// `on_start(|state| migrate)` pair: migrations run *inside* `build_state()`
/// (so a broken schema fails the boot, and `TestApp` gets them too, which the
/// serve-only `on_start` never did), the live-URL rotation loop is started at
/// serve time, and the pool is closed on graceful shutdown.
///
/// # `datasource.enabled`
///
/// There is **no** enabled gate: a pool bean has no inert form — the graph
/// promises a `DbPool` and every consumer would fail on it anyway. Setting
/// `<prefix>.enabled = false` only logs a warning and is otherwise ignored
/// (same call as the [`Executor`](https://docs.rs/r2e-executor) plugin's).
/// To point an app at a different database, change `datasource.url`; to replace
/// it wholesale in a test, pin the pool (`override_bean`) — see
/// [`SKIP_BUILD_WHEN_ALL_PINNED`](Plugin::SKIP_BUILD_WHEN_ALL_PINNED).
pub struct SqlxDataSource<DB: Database, Tag = DefaultDataSource> {
    migrator: Option<&'static Migrator>,
    marker: PhantomData<fn() -> (DB, Tag)>,
}

impl<DB: Database, Tag> Default for SqlxDataSource<DB, Tag> {
    fn default() -> Self {
        Self::new()
    }
}

impl<DB: Database, Tag> SqlxDataSource<DB, Tag> {
    /// A datasource with no migrator: `migrate-at-start` has nothing to run.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            migrator: None,
            marker: PhantomData,
        }
    }

    /// Attach the compile-time migration set (`sqlx::migrate!("./migrations")`).
    ///
    /// Attaching it does not run it: `datasource.migrate-at-start: true` does
    /// (Quarkus' `quarkus.flyway.migrate-at-start`), so the same binary can
    /// migrate in dev and stay read-only in production.
    #[must_use]
    pub const fn migrations(mut self, migrator: &'static Migrator) -> Self {
        self.migrator = Some(migrator);
        self
    }
}

impl<DB, Tag> Plugin for SqlxDataSource<DB, Tag>
where
    DB: Database,
    Tag: DataSourceTag,
    DB::Connection: Migrate,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
{
    type Provided = (DbPool<DB, Tag>,);
    /// The registry is what makes the URL *live*: the pool subscribes to
    /// `<prefix>.url` through it and rotates when a provider publishes a new
    /// value. It arrives with `load_config()`, so an app without config gets
    /// the ordinary missing-bean error at `build_state()`.
    type Deps = (LiveConfigRegistry,);
    type Config = DataSourceConfig;
    type Controllers = ();
    const CONFIG_PREFIX: Option<&'static str> = Some(Tag::CONFIG_PREFIX);

    /// `build` produces exactly one bean, and both effects it registers act on
    /// the pool `build` itself connects — the rotation loop that drives it and
    /// the close that disposes of it. A test that pins the pool has replaced
    /// all of that, so skipping the connection (and the migrations) is exactly
    /// right.
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

        let mut options = PoolOptions::<DB>::new();
        if let Some(max) = config.max_connections {
            options = options.max_connections(max);
        }
        if let Some(min) = config.min_connections {
            options = options.min_connections(min);
        }
        if let Some(timeout) = config.acquire_timeout {
            options = options.acquire_timeout(timeout);
        }

        // The URL is read through the live registry, not copied out of
        // `config`: same key, but the handle keeps rotating with it.
        let url: LiveConfig<String> = live.live_config(format!("{prefix}.url"));
        let pool = DbPool::<DB, Tag>::connect_with(options, url).await?;

        match (config.migrate_at_start, self.migrator) {
            (true, Some(migrator)) => {
                let target = pool.current();
                migrator.run(&target).await?;
                tracing::info!(datasource = prefix, "database migrations applied");
            }
            (true, None) => tracing::warn!(
                "`{prefix}.migrate-at-start` is true but no migration set was attached: \
                 call `.migrations(&MIGRATOR)` on the plugin (`sqlx::migrate!(\"./migrations\")`)"
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
            serve.track_named("sqlx pool health watcher", async move {
                r2e_core::ServiceComponent::start(watcher, shutdown).await;
            });
        });

        let closing = pool.clone();
        ctx.on_shutdown_async(move || async move {
            closing.current().close().await;
        });

        Ok((pool,))
    }
}
