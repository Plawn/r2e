//! Integration tests for the PostgreSQL example.
//!
//! A real PostgreSQL server is provided by `DevPostgres` (testcontainers) —
//! no local Postgres install, just a running Docker daemon. The test boots the
//! actual `PostgresApp` against the container and exercises the CRUD endpoints
//! over HTTP.
//!
//! Requires Docker; `#[ignore]`d by default:
//!
//! ```bash
//! cargo test -p example-postgres --test postgres_test -- --ignored
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

use example_postgres::models::Article;
use example_postgres::PostgresApp;
use r2e_devservices::DevPostgres;
use r2e_test::TestApp;
use sqlx::{AssertSqlSafe, Connection};

/// Boot the app against the shared dev PostgreSQL container.
///
/// `DevPostgres::shared()` reuses one container across every test process in
/// the suite, so tests must NOT assume an empty database. We isolate each test
/// by creating a fresh database on that container and pointing the app at it
/// via `override_config_value("database.url", ...)` — the same key the app
/// reads in its `#[producer]`.
///
/// The app applies migrations in its `on_start` hook, but `on_start` runs only
/// on the serve path, not under `TestApp` — so the schema is applied here,
/// against the isolated database, reusing the app's own migration set.
async fn boot() -> TestApp {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let pg = DevPostgres::shared().await;

    // `pg.url()` targets the container's default `postgres` database; split off
    // the trailing `/postgres` to get the server base, then a unique per-test db.
    let base = pg
        .url()
        .rsplit_once('/')
        .expect("dev postgres url has a /database segment")
        .0;
    let db = format!(
        "r2e_pg_test_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );

    // CREATE DATABASE runs on the default database (it cannot run in a tx).
    let mut admin = sqlx::PgConnection::connect(pg.url())
        .await
        .expect("connect to the shared postgres database");
    // The db name is derived from pid/counter, not user input; assert it safe.
    let create = format!("CREATE DATABASE \"{db}\"");
    sqlx::raw_sql(AssertSqlSafe(create))
        .execute(&mut admin)
        .await
        .expect("create the isolated test database");
    admin.close().await.ok();

    let url = format!("{base}/{db}");

    // Same migration set the app runs in `on_start` (which TestApp doesn't fire).
    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("connect to the isolated test database");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    pool.close().await;

    TestApp::boot_with::<PostgresApp>(move |b| b.override_config_value("database.url", url)).await
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn list_is_empty_on_a_fresh_database() {
    let app = boot().await;
    let resp = app.get("/articles?page=0&size=20").send().await;
    resp.assert_ok();
    resp.assert_json_path("total_elements", 0);
    resp.assert_json_path("content", serde_json::json!([]));
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn create_then_fetch_roundtrips_through_postgres() {
    let app = boot().await;

    let created = app
        .post("/articles")
        .json(&serde_json::json!({ "title": "Hello", "body": "world", "published": true }))
        .send()
        .await;
    created.assert_ok();
    let created: Article = created.json();
    assert_eq!(created.title, "Hello");
    assert!(created.published);

    let fetched = app.get(&format!("/articles/{}", created.id)).send().await;
    fetched.assert_ok();
    let fetched: Article = fetched.json();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.body, "world");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn update_persists_partial_changes() {
    let app = boot().await;

    let created: Article = app
        .post("/articles")
        .json(&serde_json::json!({ "title": "Draft", "body": "body" }))
        .send()
        .await
        .json();
    assert!(!created.published);

    // Only `published` is set — title and body keep their stored values.
    let updated: Article = app
        .put(&format!("/articles/{}", created.id))
        .json(&serde_json::json!({ "published": true }))
        .send()
        .await
        .json();
    assert!(updated.published);
    assert_eq!(updated.title, "Draft");
    assert_eq!(updated.body, "body");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn delete_removes_the_row() {
    let app = boot().await;

    let created: Article = app
        .post("/articles")
        .json(&serde_json::json!({ "title": "Temp", "body": "gone soon" }))
        .send()
        .await
        .json();

    app.delete(&format!("/articles/{}", created.id))
        .send()
        .await
        .assert_ok();

    app.get(&format!("/articles/{}", created.id))
        .send()
        .await
        .assert_not_found();
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn missing_article_is_not_found() {
    let app = boot().await;
    app.get("/articles/999999").send().await.assert_not_found();
}
