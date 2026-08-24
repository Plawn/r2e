use std::time::Duration;

use r2e_core::ServiceComponent;
use r2e_data_sqlx::DbPool;
use sqlx::{Error, Executor, Row, Sqlite};
use r2e_core::rt::CancelToken;

use crate::support::{cleanup_sqlite_file, live_url, sqlite_file_url};

#[tokio::test]
async fn rotating_pool_swaps_to_updated_live_config_url() {
    let initial_url = sqlite_file_url("rotate-initial");
    let rotated_url = sqlite_file_url("rotate-rotated");
    let (url, registry) = live_url("db.url", &initial_url);
    let pool = DbPool::<Sqlite>::connect(url).await.unwrap();

    (&pool)
        .execute("CREATE TABLE items(id INTEGER PRIMARY KEY)")
        .await
        .unwrap();
    let token = CancelToken::new();
    let service = pool.clone();
    let service_token = token.clone();
    let handle = r2e_core::rt::spawn(async move {
        service.start(service_token).await;
    });

    assert!(registry.set("db.url", rotated_url.as_str()));
    for _ in 0..50 {
        if pool.generation() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    token.cancel();
    handle.await.unwrap();

    assert_eq!(pool.generation(), 1);
    assert!(pool.last_error().is_none());
    let result = (&pool)
        .execute("CREATE TABLE rotated(id INTEGER PRIMARY KEY)")
        .await;
    assert!(
        result.is_ok(),
        "rotated pool should remain executable: {result:?}"
    );

    cleanup_sqlite_file(&initial_url);
    cleanup_sqlite_file(&rotated_url);
}

/// The pool and the generation come out of one atomic cell, so the generation
/// always describes the pool handed out with it — never the one it replaced.
#[tokio::test]
async fn snapshot_pairs_the_active_pool_with_its_generation() {
    let initial_url = sqlite_file_url("snapshot-initial");
    let rotated_url = sqlite_file_url("snapshot-rotated");
    let (url, _registry) = live_url("db.url", &initial_url);
    let pool = DbPool::<Sqlite>::connect(url).await.unwrap();

    let (initial, initial_generation) = pool.snapshot();
    assert_eq!(initial_generation, 0);
    assert_eq!(pool.generation(), 0);
    initial
        .execute("CREATE TABLE only_in_generation_zero(id INTEGER PRIMARY KEY)")
        .await
        .unwrap();

    pool.rotate_to(rotated_url.as_str()).await.unwrap();

    let (rotated, rotated_generation) = pool.snapshot();
    assert_eq!(rotated_generation, 1);
    assert_eq!(pool.generation(), 1);
    // Same read, so `current()` must agree with the snapshot's pool: the
    // generation-1 database is a different file and has no such table.
    let current = pool.current();
    assert_eq!(table_count(&current, "only_in_generation_zero").await, 0);
    assert_eq!(table_count(&rotated, "only_in_generation_zero").await, 0);

    cleanup_sqlite_file(&initial_url);
    cleanup_sqlite_file(&rotated_url);
}

/// A rotation closes the pool it replaces, so any handle taken just before the
/// swap fails with `PoolClosed`. `DbPool::begin` re-snapshots and retries, so
/// callers never see that as a request failure.
#[tokio::test]
async fn begin_retries_onto_the_pool_that_replaced_a_closed_one() {
    let initial_url = sqlite_file_url("retry-initial");
    let rotated_url = sqlite_file_url("retry-rotated");
    let (url, _registry) = live_url("db.url", &initial_url);
    let pool = DbPool::<Sqlite>::connect(url).await.unwrap();

    // Exactly what the torn-snapshot code used to hand to `begin()`.
    let (stale, stale_generation) = pool.snapshot();
    assert_eq!(stale_generation, 0);

    pool.rotate_to(rotated_url.as_str()).await.unwrap();

    // The old pool is closed by a spawned task; wait for it so the failure
    // mode below is deterministic rather than a race.
    for _ in 0..200 {
        if stale.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        stale.is_closed(),
        "rotation should close the pool it replaced"
    );
    assert!(
        matches!(stale.begin().await, Err(Error::PoolClosed)),
        "a pre-rotation pool handle must be the thing that fails"
    );

    let (transaction, generation) = pool
        .begin()
        .await
        .expect("the facade should retry onto the rotated pool");
    assert_eq!(generation, 1);
    transaction.rollback().await.unwrap();

    // The `Executor for &DbPool` path snapshots at poll time, so it is covered
    // by the same rotation.
    (&pool)
        .execute("CREATE TABLE after_rotation(id INTEGER PRIMARY KEY)")
        .await
        .expect("executing through the facade should use the rotated pool");

    cleanup_sqlite_file(&initial_url);
    cleanup_sqlite_file(&rotated_url);
}

async fn table_count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    sqlx::query("SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name = ?")
        .bind(table)
        .fetch_one(pool)
        .await
        .unwrap()
        .get::<i64, _>("count")
}
