use r2e_core::http::StatusCode;
use r2e_core::{AppBuilder, BeanLookup, ManagedContext, ManagedGuard, ManagedOutcome};
use r2e_data_sqlx::{DbPool, DbTx, SqlxTx};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Executor, Row, Sqlite, SqlitePool};

use crate::support::{cleanup_sqlite_file, live_url, sqlite_file_url};

const CREATE_ITEMS: &str = "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL)";

async fn pool_with_table() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(CREATE_ITEMS).execute(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn commits_successful_outcome() {
    let app = AppBuilder::new()
        .provide(pool_with_table().await)
        .build_state()
        .await;
    let mut tx = ManagedGuard::<SqlxTx<'static, Sqlite>, _>::acquire(ManagedContext::new(
        app.state(),
        "Test",
        "commit",
    ))
    .await
    .unwrap();
    sqlx::query("INSERT INTO items(name) VALUES ('committed')")
        .execute(tx.resource_mut().connection())
        .await
        .unwrap();
    tx.finalize(&ManagedOutcome::from_status(StatusCode::CREATED))
        .await
        .unwrap();

    let pool = app.state().bean::<SqlitePool>().unwrap();
    let row = sqlx::query("SELECT COUNT(*) AS count FROM items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("count"), 1);
}

#[tokio::test]
async fn rolls_back_failure_outcome() {
    let app = AppBuilder::new()
        .provide(pool_with_table().await)
        .build_state()
        .await;
    let mut tx = ManagedGuard::<SqlxTx<'static, Sqlite>, _>::acquire(ManagedContext::new(
        app.state(),
        "Test",
        "rollback",
    ))
    .await
    .unwrap();
    sqlx::query("INSERT INTO items(name) VALUES ('rolled back')")
        .execute(tx.resource_mut().connection())
        .await
        .unwrap();
    tx.finalize(&ManagedOutcome::from_status(StatusCode::BAD_REQUEST))
        .await
        .unwrap();

    let pool = app.state().bean::<SqlitePool>().unwrap();
    let row = sqlx::query("SELECT COUNT(*) AS count FROM items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("count"), 0);
}

/// `DbTx::generation()` must describe the pool the transaction actually ran on.
/// Only the `items` table exists in the generation-0 database, so the row count
/// from `sqlite_master` is a direct witness of which pool served the
/// transaction.
#[tokio::test]
async fn rotating_transaction_reports_the_generation_it_ran_on() {
    let initial_url = sqlite_file_url("tx-initial");
    let rotated_url = sqlite_file_url("tx-rotated");
    let (url, _registry) = live_url("db.url", &initial_url);
    let pool = DbPool::<Sqlite>::connect(url).await.unwrap();
    (&pool).execute(CREATE_ITEMS).await.unwrap();

    let app = AppBuilder::new().provide(pool.clone()).build_state().await;

    let mut before = ManagedGuard::<DbTx<'static, Sqlite>, _>::acquire(ManagedContext::new(
        app.state(),
        "Test",
        "before_rotation",
    ))
    .await
    .unwrap();
    assert_eq!(before.resource_mut().generation(), 0);
    assert_eq!(
        items_table_count(before.resource_mut().connection()).await,
        1,
        "generation 0 must run on the initial database"
    );
    before
        .finalize(&ManagedOutcome::from_status(StatusCode::OK))
        .await
        .unwrap();

    pool.rotate_to(rotated_url.as_str()).await.unwrap();

    let mut after = ManagedGuard::<DbTx<'static, Sqlite>, _>::acquire(ManagedContext::new(
        app.state(),
        "Test",
        "after_rotation",
    ))
    .await
    .unwrap();
    assert_eq!(after.resource_mut().generation(), 1);
    assert_eq!(
        items_table_count(after.resource_mut().connection()).await,
        0,
        "generation 1 must run on the rotated database"
    );
    after
        .finalize(&ManagedOutcome::from_status(StatusCode::OK))
        .await
        .unwrap();

    cleanup_sqlite_file(&initial_url);
    cleanup_sqlite_file(&rotated_url);
}

/// Does the database this connection is bound to have the `items` table?
async fn items_table_count(connection: &mut sqlx::SqliteConnection) -> i64 {
    sqlx::query(
        "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'items'",
    )
    .fetch_one(connection)
    .await
    .unwrap()
    .get::<i64, _>("count")
}
