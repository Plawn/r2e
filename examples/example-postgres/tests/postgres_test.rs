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

use example_postgres::models::Article;
use example_postgres::PostgresApp;
use r2e_devservices::DevPostgres;
use r2e_test::{TestApp, TestResource};

/// The database a test runs against.
///
/// `DevPostgres::shared()` reuses one container across every test process of
/// the session, so tests must NOT assume an empty database: each one takes a
/// `fresh_database()` on that container and points the app at it through
/// `datasource.url` — the key the `SqlxDataSource` plugin reads.
///
/// Migrations are the plugin's job: it runs them inside `build_state()`, so
/// the fresh database gets its schema without any help from the test.
struct Db(DevPostgres);

impl TestResource for Db {
    async fn start() -> Self {
        Db(DevPostgres::shared().await.fresh_database().await)
    }

    fn config(&self) -> Vec<(String, String)> {
        vec![("datasource.url".into(), self.0.url().into())]
    }
}

#[r2e::test(app = PostgresApp, resources(Db))]
#[ignore = "requires Docker"]
async fn list_is_empty_on_a_fresh_database(app: TestApp) {
    let resp = app.get("/articles?page=0&size=20").send().await;
    resp.assert_ok();
    resp.assert_json_path("total_elements", 0);
    resp.assert_json_path("content", serde_json::json!([]));
}

#[r2e::test(app = PostgresApp, resources(Db))]
#[ignore = "requires Docker"]
async fn create_then_fetch_roundtrips_through_postgres(app: TestApp) {
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

#[r2e::test(app = PostgresApp, resources(Db))]
#[ignore = "requires Docker"]
async fn update_persists_partial_changes(app: TestApp) {
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

#[r2e::test(app = PostgresApp, resources(Db))]
#[ignore = "requires Docker"]
async fn delete_removes_the_row(app: TestApp) {
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

#[r2e::test(app = PostgresApp, resources(Db))]
#[ignore = "requires Docker"]
async fn missing_article_is_not_found(app: TestApp) {
    app.get("/articles/999999").send().await.assert_not_found();
}
