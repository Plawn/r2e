//! Per-test rollback (`@TestTransaction`): the app is booted on a
//! rollback-only pool pinned by a `TestResource`. Its routes — managed
//! transactions included — write for real, and nothing is ever committed.

use example_app::models::User;
use r2e::prelude::*;
use r2e::r2e_data_sqlx::rollback_only_pool;
use r2e_test::{TestApp, TestResource};
use sqlx::{Connection, Sqlite, SqliteConnection, SqlitePool};

/// A migrated, seeded database file and the rollback-only pool over it.
struct RollbackDb {
    url: String,
    path: std::path::PathBuf,
    pool: SqlitePool,
}

impl RollbackDb {
    /// Rows as an ordinary connection sees them: what was committed.
    async fn committed_users(&self) -> i64 {
        let mut conn = SqliteConnection::connect(&self.url).await.unwrap();
        sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&mut conn)
            .await
            .unwrap()
    }
}

impl TestResource for RollbackDb {
    async fn start() -> Self {
        let path = std::env::temp_dir().join(format!(
            "r2e-example-rollback-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let url = format!("sqlite://{}?mode=rwc", path.display());

        // Schema and seed go through an ordinary connection: they are
        // committed, the test's writes are not.
        let mut setup = SqliteConnection::connect(&url).await.unwrap();
        sqlx::query(
            "CREATE TABLE users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                email TEXT NOT NULL,
                sub TEXT
            )",
        )
        .execute(&mut setup)
        .await
        .unwrap();
        sqlx::query("INSERT INTO users (name, email) VALUES ('Seed', 'seed@example.com')")
            .execute(&mut setup)
            .await
            .unwrap();
        setup.close().await.unwrap();

        let pool = rollback_only_pool::<Sqlite>(&url).await.unwrap();
        RollbackDb { url, path, pool }
    }

    fn configure(&self, builder: AppBuilder) -> AppBuilder {
        builder.override_bean(self.pool.clone())
    }
}

impl Drop for RollbackDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[r2e::test(app = example_app::ExampleApp, resources(RollbackDb))]
async fn a_managed_transaction_of_a_route_is_never_committed(
    app: TestApp,
    #[inject] pool: SqlitePool,
) {
    // `POST /users/db` runs in a `#[managed]` transaction, committed on `Ok`.
    let resp = app
        .post("/users/db")
        .as_user("user-1", &["user"])
        .json(&serde_json::json!({"name": "Dana", "email": "dana@example.com"}))
        .send()
        .await;
    resp.assert_ok();
    let created: User = resp.json();
    assert_eq!(created.name, "Dana");

    // The app — and the test, through the same bean — sees the write…
    let seen: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(seen, 2);

    // …and the database never received it.
    assert_eq!(app.resource::<RollbackDb>().committed_users().await, 1);
}
