use http_body_util::BodyExt;
use r2e_core::builtins::Health;
use r2e_core::serde_json;
use r2e_core::http::{Body, Request, Router, StatusCode};
use r2e_core::{AppBuilder, R2eConfig};
use r2e_data_sqlx::{DataSourceHealth, SqlxDataSource};
use sqlx::Sqlite;
use tower::ServiceExt;

use crate::support::{cleanup_sqlite_file, sqlite_file_url};

r2e_data_sqlx::datasource_tag!(
    /// A second datasource, so the check's default name carries its tag.
    pub Reporting = "reporting"
);

fn config(url: &str) -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("datasource.url", url.into());
    config
}

/// `GET path` → `(status, body)`.
async fn get(router: Router, path: &str) -> (StatusCode, String) {
    let resp = router
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn a_live_pool_reports_up_on_health_and_ready() {
    let url = sqlite_file_url("health-up");

    let router = AppBuilder::new()
        .override_config(config(&url))
        .load_config::<()>()
        .plugin(SqlxDataSource::<Sqlite>::new())
        // Install order is irrelevant: the registry is a bean, and the routes
        // are only assembled in the Routes stage.
        .plugin(DataSourceHealth::<Sqlite>::new())
        .plugin(Health::builder().build())
        .build_state()
        .await
        .build();

    let (status, body) = get(router.clone(), "/health").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
    assert_eq!(json["checks"][0]["name"], "db");
    assert_eq!(json["checks"][0]["status"], "UP");

    let (status, _) = get(router, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);

    cleanup_sqlite_file(&url);
}

/// A named datasource names its check after its tag.
#[tokio::test]
async fn a_named_datasource_names_its_check_after_its_tag() {
    let url = sqlite_file_url("health-named");
    let mut config = config(&url);
    config.set("datasource.reporting.url", url.clone().into());

    let router = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .plugin(SqlxDataSource::<Sqlite, Reporting>::new())
        .plugin(DataSourceHealth::<Sqlite, Reporting>::new().liveness_only())
        .plugin(Health::builder().build())
        .build_state()
        .await
        .build();

    let (status, body) = get(router.clone(), "/health").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["checks"][0]["name"], "db:reporting");
    assert_eq!(json["checks"][0]["status"], "UP");

    // `liveness_only()` keeps the check out of readiness aggregation; with a
    // live pool both probes are green either way.
    let (status, _) = get(router, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);

    cleanup_sqlite_file(&url);
}
