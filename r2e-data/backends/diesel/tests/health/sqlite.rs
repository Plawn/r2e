use diesel::SqliteConnection;
use r2e_core::builtins::health::{HealthCheckStatus, HealthRegistry};
use r2e_core::builtins::Health;
use r2e_core::{AppBuilder, BeanAccess, R2eConfig};
use r2e_data_diesel::{DataSourceHealth, DieselDataSource};

use crate::support::{cleanup_sqlite_file, sqlite_file_path};

r2e_data_diesel::datasource_tag!(
    /// A second datasource, so the check's default name carries its tag.
    pub Reporting = "reporting"
);

fn config(path: &str) -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("datasource.url", path.into());
    config.set("datasource.reporting.url", path.into());
    config
}

/// Boot a default datasource plus `health`, and hand back the registry the
/// check was contributed to.
async fn registry_with(path: &str, health: DataSourceHealth<SqliteConnection>) -> HealthRegistry {
    let app = AppBuilder::new()
        .override_config(config(path))
        .load_config::<()>()
        .plugin(DieselDataSource::<SqliteConnection>::new())
        .plugin(health)
        .plugin(Health::builder().build())
        .build_state()
        .await;
    app.state().get::<HealthRegistry>()
}

/// The default check is named `db`, reports a live pool as up, and counts
/// towards readiness.
#[r2e_core::test]
async fn default_check_is_named_db_and_gates_readiness() {
    let path = sqlite_file_path("health-default");
    let registry = registry_with(&path, DataSourceHealth::new()).await;

    assert_eq!(registry.names(), ["db"]);

    let state = registry.into_state();
    let health = state.aggregate().await;
    assert_eq!(health.checks.len(), 1);
    assert_eq!(health.checks[0].name, "db");
    assert!(matches!(health.checks[0].status, HealthCheckStatus::Up));

    let ready = state.aggregate_readiness().await;
    let names: Vec<_> = ready.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        ["db"],
        "the default check must count towards readiness"
    );

    cleanup_sqlite_file(&path);
}

/// `liveness_only()` keeps the check on `/health` but out of `/health/ready`.
#[r2e_core::test]
async fn liveness_only_check_is_left_out_of_readiness() {
    let path = sqlite_file_path("health-liveness");
    let registry = registry_with(&path, DataSourceHealth::new().liveness_only()).await;

    let state = registry.into_state();
    assert_eq!(state.aggregate().await.checks.len(), 1);
    assert!(
        state.aggregate_readiness().await.checks.is_empty(),
        "a liveness-only check must not count towards readiness"
    );

    cleanup_sqlite_file(&path);
}

/// `named(..)` overrides the default `db` name.
#[r2e_core::test]
async fn named_overrides_the_check_name() {
    let path = sqlite_file_path("health-named");
    let registry = registry_with(&path, DataSourceHealth::new().named("primary-store")).await;

    assert_eq!(registry.names(), ["primary-store"]);

    cleanup_sqlite_file(&path);
}

/// A named datasource names its check `db:<tag>`.
#[r2e_core::test]
async fn tagged_datasource_names_its_check_after_its_tag() {
    let path = sqlite_file_path("health-tagged");
    let app = AppBuilder::new()
        .override_config(config(&path))
        .load_config::<()>()
        .plugin(DieselDataSource::<SqliteConnection, Reporting>::new())
        .plugin(DataSourceHealth::<SqliteConnection, Reporting>::new())
        .plugin(Health::builder().build())
        .build_state()
        .await;

    assert_eq!(
        app.state().get::<HealthRegistry>().names(),
        ["db:reporting"]
    );

    cleanup_sqlite_file(&path);
}
