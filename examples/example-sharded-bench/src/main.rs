//! Minimal r2e app for benchmarking SO_REUSEPORT sharded serving.
//!
//! Three GET endpoints:
//!   - `/plain` → plaintext "Hello, World!"
//!   - `/json`  → small serialized JSON object
//!   - `/db`    → one sqlite SELECT by id, returning a small JSON row
//!
//! The number of workers is switched at runtime via the `R2E_SERVER_WORKERS`
//! environment variable (config overlay → `server.workers`), so the same
//! release binary serves both the default multi-thread mode (var unset) and
//! the sharded mode (`R2E_SERVER_WORKERS=per-core`) without rebuilding.
//!
//! See `tools/bench-sharded.sh` and `docs/features/19-sharded-serving.md`.

use r2e::prelude::*;
use sqlx::sqlite::SqlitePoolOptions;

mod controllers;

use controllers::BenchController;

/// Create a FILE-backed sqlite pool and seed a small table.
///
/// A file (not `sqlite::memory:`) is mandatory: with a connection pool, each
/// pooled connection to `:memory:` gets its own SEPARATE empty database, so a
/// row seeded on one connection is invisible to the next. A file in the system
/// temp dir is shared by every pooled connection. The file is created fresh on
/// each startup (the previous one, if any, is removed first).
async fn make_pool() -> sqlx::SqlitePool {
    let db_path = std::env::temp_dir().join("r2e-sharded-bench.db");
    // Start from a clean slate so reseeding is deterministic.
    let _ = std::fs::remove_file(&db_path);

    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(16)
        .connect(&url)
        .await
        .expect("failed to open sqlite pool");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS items (
            id    INTEGER PRIMARY KEY,
            name  TEXT NOT NULL,
            value INTEGER NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .expect("create table");

    // Seed 100 rows.
    for id in 1..=100i64 {
        sqlx::query("INSERT OR REPLACE INTO items (id, name, value) VALUES (?, ?, ?)")
            .bind(id)
            .bind(format!("item-{id}"))
            .bind(id * 10)
            .execute(&pool)
            .await
            .expect("seed row");
    }

    pool
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let pool = make_pool().await;

    // Config: application.yaml (host/port) overlaid with R2E_-prefixed env vars.
    // `R2E_SERVER_WORKERS` maps to `server.workers` and selects the serve mode.
    AppBuilder::new()
        .load_config::<()>()
        .provide(pool)
        .build_state()
        .await
        .register_controller::<BenchController>()
        .serve_auto()
        .await
        .expect("serve");
}
