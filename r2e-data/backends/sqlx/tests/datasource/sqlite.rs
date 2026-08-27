use r2e_core::{AppBuilder, BeanAccess, R2eConfig};
use r2e_data_sqlx::{DbPool, SqlxDataSource};
use sqlx::{Pool, Sqlite};

use crate::support::{cleanup_sqlite_file, live_url, sqlite_file_url};

/// The migration set every "it applied" assertion is written against: it
/// creates `items`, and nothing else does.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("tests/fixtures/migrations");

/// A migration set whose SQL cannot parse — the failure path.
static BROKEN_MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("tests/fixtures/broken-migrations");

r2e_data_sqlx::datasource_tag!(
    /// A second datasource, reading `datasource.reporting.*` and providing
    /// `DbPool<Sqlite, Reporting>`.
    pub Reporting = "reporting"
);

/// `datasource:` with the URL and the migrate flag the test wants.
fn config(url: &str, migrate_at_start: bool) -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("datasource.url", url.into());
    config.set("datasource.migrate-at-start", migrate_at_start.into());
    config
}

/// Whether the migrator's table exists — the proof that migrations ran.
async fn has_items_table(pool: &Pool<Sqlite>) -> bool {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='items'")
            .fetch_one(pool)
            .await
            .unwrap();
    count == 1
}

#[tokio::test]
async fn connects_and_migrates_at_boot() {
    let url = sqlite_file_url("ds-migrate");

    let app = AppBuilder::new()
        .override_config(config(&url, true))
        .load_config::<()>()
        .plugin(SqlxDataSource::<Sqlite>::new().migrations(&MIGRATOR))
        .build_state()
        .await;

    let pool = app.state().get::<DbPool<Sqlite>>();
    assert!(
        has_items_table(&pool.current()).await,
        "migrate-at-start: true must apply the attached migrator during build_state()"
    );

    cleanup_sqlite_file(&url);
}

/// Attaching a migrator is not the same as running it: the flag decides, so the
/// same binary can migrate in dev and stay read-only in production.
#[tokio::test]
async fn migrations_are_skipped_when_migrate_at_start_is_false() {
    let url = sqlite_file_url("ds-no-migrate");

    let app = AppBuilder::new()
        .override_config(config(&url, false))
        .load_config::<()>()
        .plugin(SqlxDataSource::<Sqlite>::new().migrations(&MIGRATOR))
        .build_state()
        .await;

    let pool = app.state().get::<DbPool<Sqlite>>();
    assert!(
        !has_items_table(&pool.current()).await,
        "migrate-at-start: false must leave the database untouched"
    );

    cleanup_sqlite_file(&url);
}

/// A migration that cannot apply aborts the boot instead of leaving the app
/// running against a schema it does not have.
#[tokio::test]
async fn failing_migration_aborts_boot() {
    let url = sqlite_file_url("ds-broken-migrate");

    let result = AppBuilder::new()
        .override_config(config(&url, true))
        .load_config::<()>()
        .plugin(SqlxDataSource::<Sqlite>::new().migrations(&BROKEN_MIGRATOR))
        .try_build_state()
        .await;

    let error = match result {
        Ok(_) => panic!("a broken migration must fail build_state()"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("Plugin 'SqlxDataSource' failed to build"),
        "boot error should name the datasource plugin, got: {error}"
    );

    cleanup_sqlite_file(&url);
}

/// A named datasource is a different bean type reading a different section, so
/// two of them live in one app without ambiguity.
#[tokio::test]
async fn named_datasource_coexists_with_the_default_one() {
    let default_url = sqlite_file_url("ds-default");
    let reporting_url = sqlite_file_url("ds-reporting");

    let mut config = config(&default_url, false);
    config.set("datasource.reporting.url", reporting_url.as_str().into());
    config.set("datasource.reporting.migrate-at-start", true.into());

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(SqlxDataSource::<Sqlite>::new())
        .plugin(SqlxDataSource::<Sqlite, Reporting>::new().migrations(&MIGRATOR))
        .build_state()
        .await;

    let default = app.state().get::<DbPool<Sqlite>>();
    let reporting = app.state().get::<DbPool<Sqlite, Reporting>>();

    // Each read its own section: only `datasource.reporting` asked for
    // migrations, and only the reporting database has the table.
    assert!(has_items_table(&reporting.current()).await);
    assert!(!has_items_table(&default.current()).await);

    cleanup_sqlite_file(&default_url);
    cleanup_sqlite_file(&reporting_url);
}

/// `SKIP_BUILD_WHEN_ALL_PINNED`: a test that supplies its own pool gets no
/// connection attempt at all — proven here by a config that has no `url`,
/// which an executed `build` would reject.
#[tokio::test]
async fn pinned_pool_skips_the_connection() {
    let url = sqlite_file_url("ds-pinned");
    let (live, _registry) = live_url("pinned.url", &url);
    let pinned = DbPool::<Sqlite>::connect(live).await.unwrap();

    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .override_bean(pinned)
        .plugin(SqlxDataSource::<Sqlite>::new().migrations(&MIGRATOR))
        .build_state()
        .await;

    let pool = app.state().get::<DbPool<Sqlite>>();
    // The pinned pool is untouched: no migrations were run on it either.
    assert!(!has_items_table(&pool.current()).await);

    cleanup_sqlite_file(&url);
}

/// Without a URL and without a pin there is nothing to connect to, and the
/// error says which key is missing rather than surfacing an SQLx parse failure.
#[tokio::test]
async fn missing_url_fails_with_a_pointed_error() {
    let result = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .plugin(SqlxDataSource::<Sqlite>::new())
        .try_build_state()
        .await;

    let error = match result {
        Ok(_) => panic!("a datasource without a URL must fail build_state()"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("`datasource.url` is not set"),
        "boot error should name the missing key, got: {error}"
    );
}
