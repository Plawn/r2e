// diesel's `QueryableByName` derive expands to `field: field` with our spans.
#![allow(clippy::redundant_field_names)]

use std::time::Duration;

use diesel::r2d2::{ConnectionManager, Pool};
use diesel::{sql_query, QueryableByName, RunQueryDsl, SqliteConnection};
use r2e_core::http::StatusCode;
use r2e_core::{
    AppBuilder, BeanLookup, ManagedContext, ManagedGuard, ManagedOutcome, ManagedResource,
};
use r2e_data_diesel::{DbPool, DbTx, DieselTx};

use crate::support::{cleanup_sqlite_file, live_url, sqlite_file_path};

const CREATE_ITEMS: &str = "CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL)";

const COUNT_ITEMS_TABLE: &str =
    "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name = 'items'";

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

fn pool_with_table() -> Pool<ConnectionManager<SqliteConnection>> {
    let manager = ConnectionManager::<SqliteConnection>::new(":memory:");
    let pool = Pool::builder().max_size(1).build(manager).unwrap();
    sql_query(CREATE_ITEMS)
        .execute(&mut pool.get().unwrap())
        .unwrap();
    pool
}

#[tokio::test]
async fn commits_and_rolls_back_from_http_outcome() {
    let app = AppBuilder::new()
        .provide(pool_with_table())
        .build_state()
        .await;
    let mut committed = ManagedGuard::<DieselTx<SqliteConnection>, _>::acquire(
        ManagedContext::new(app.state(), "Test", "commit"),
    )
    .await
    .unwrap();
    committed
        .resource_mut()
        .run(|connection| {
            sql_query("INSERT INTO items(name) VALUES ('committed')").execute(connection)
        })
        .await
        .unwrap();
    committed
        .finalize(&ManagedOutcome::from_status(StatusCode::CREATED))
        .await
        .unwrap();

    let mut rolled_back = ManagedGuard::<DieselTx<SqliteConnection>, _>::acquire(
        ManagedContext::new(app.state(), "Test", "rollback"),
    )
    .await
    .unwrap();
    rolled_back
        .resource_mut()
        .run(|connection| {
            sql_query("INSERT INTO items(name) VALUES ('rolled back')").execute(connection)
        })
        .await
        .unwrap();
    rolled_back
        .finalize(&ManagedOutcome::from_status(StatusCode::BAD_REQUEST))
        .await
        .unwrap();

    let pool = app
        .state()
        .bean::<Pool<ConnectionManager<SqliteConnection>>>()
        .unwrap();
    let mut connection = pool.get().unwrap();
    let count = sql_query("SELECT COUNT(*) AS count FROM items")
        .get_result::<Count>(&mut connection)
        .unwrap();
    assert_eq!(count.count, 1);
}

/// `abort` is the synchronous fallback for a handler that never reached
/// `finalize` (panic, cancellation): it must give the connection — still
/// inside its open transaction — back *now*, so r2d2 discards it and the
/// uncommitted row is gone. The pool has a single slot, so a connection the
/// aborted transaction kept would make the next checkout time out.
#[r2e_core::test]
async fn abort_releases_the_connection_and_discards_the_open_transaction() {
    let path = sqlite_file_path("tx-abort");
    let pool = Pool::builder()
        .max_size(1)
        .connection_timeout(Duration::from_millis(500))
        .build(ConnectionManager::<SqliteConnection>::new(path.as_str()))
        .unwrap();
    sql_query(CREATE_ITEMS)
        .execute(&mut pool.get().unwrap())
        .unwrap();
    let app = AppBuilder::new().provide(pool.clone()).build_state().await;

    let mut guard = ManagedGuard::<DieselTx<SqliteConnection>, _>::acquire(ManagedContext::new(
        app.state(),
        "Test",
        "abort",
    ))
    .await
    .unwrap();
    guard
        .resource_mut()
        .run(|connection| {
            sql_query("INSERT INTO items(name) VALUES ('aborted')").execute(connection)
        })
        .await
        .unwrap();

    abort(&mut guard);

    // The guard is still alive: only `abort` can have freed the single slot.
    let mut connection = pool
        .get()
        .expect("abort must hand the transaction's connection back to the pool");
    let count = sql_query("SELECT COUNT(*) AS count FROM items")
        .get_result::<Count>(&mut connection)
        .unwrap();
    assert_eq!(
        count.count, 0,
        "the aborted transaction must not be committed"
    );
    drop(connection);

    // Nothing is left to finalize once aborted.
    guard
        .finalize(&ManagedOutcome::from_status(StatusCode::OK))
        .await
        .unwrap();

    cleanup_sqlite_file(&path);
}

fn abort<R, S>(guard: &mut ManagedGuard<R, S>)
where
    R: ManagedResource<S>,
{
    guard.resource_mut().abort();
}

/// `DbTx::generation()` must describe the pool the transaction actually ran on.
/// Only the `items` table exists in the generation-0 database, so the row count
/// from `sqlite_master` is a direct witness of which pool served the
/// transaction.
#[tokio::test]
async fn rotating_transaction_reports_the_generation_it_ran_on() {
    let initial_url = sqlite_file_path("tx-initial");
    let rotated_url = sqlite_file_path("tx-rotated");
    let (url, _registry) = live_url("db.url", &initial_url);
    let pool = DbPool::<SqliteConnection>::connect(url).await.unwrap();
    sql_query(CREATE_ITEMS)
        .execute(&mut pool.current().get().unwrap())
        .unwrap();

    let app = AppBuilder::new().provide(pool.clone()).build_state().await;

    let mut before = ManagedGuard::<DbTx<SqliteConnection>, _>::acquire(ManagedContext::new(
        app.state(),
        "Test",
        "before_rotation",
    ))
    .await
    .unwrap();
    assert_eq!(before.resource_mut().generation(), 0);
    assert_eq!(
        items_table_count(&mut before).await,
        1,
        "generation 0 must run on the initial database"
    );
    before
        .finalize(&ManagedOutcome::from_status(StatusCode::OK))
        .await
        .unwrap();

    pool.rotate_to(rotated_url.as_str()).await.unwrap();

    let mut after = ManagedGuard::<DbTx<SqliteConnection>, _>::acquire(ManagedContext::new(
        app.state(),
        "Test",
        "after_rotation",
    ))
    .await
    .unwrap();
    assert_eq!(after.resource_mut().generation(), 1);
    assert_eq!(
        items_table_count(&mut after).await,
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

/// Does the database this transaction is bound to have the `items` table?
async fn items_table_count<S>(guard: &mut ManagedGuard<DbTx<SqliteConnection>, S>) -> i64
where
    S: BeanLookup + Send + Sync,
{
    guard
        .resource_mut()
        .run(|connection| {
            sql_query(COUNT_ITEMS_TABLE)
                .get_result::<Count>(connection)
                .map(|row| row.count)
        })
        .await
        .unwrap()
}
