use std::time::Duration;

use diesel::r2d2::{ConnectionManager, Pool};
use diesel::{sql_query, QueryableByName, RunQueryDsl, SqliteConnection};
use r2e_core::ServiceComponent;
use r2e_data_diesel::DbPool;
use tokio_util::sync::CancellationToken;

use crate::support::{cleanup_sqlite_file, live_url, sqlite_file_path};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

#[tokio::test]
async fn rotating_pool_swaps_to_updated_live_config_url() {
    let initial_url = sqlite_file_path("rotate-initial");
    let rotated_url = sqlite_file_path("rotate-rotated");
    let (url, registry) = live_url("db.url", &initial_url);
    let pool = DbPool::<SqliteConnection>::connect(url).await.unwrap();

    let token = CancellationToken::new();
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

    // The rotated pool must still hand out working connections.
    let mut connection = pool.current().get().unwrap();
    sql_query("CREATE TABLE rotated(id INTEGER PRIMARY KEY)")
        .execute(&mut connection)
        .unwrap();

    cleanup_sqlite_file(&initial_url);
    cleanup_sqlite_file(&rotated_url);
}

/// The pool and the generation come out of one atomic cell, so the generation
/// always describes the pool handed out with it — never the one it replaced.
#[tokio::test]
async fn snapshot_pairs_the_active_pool_with_its_generation() {
    let initial_url = sqlite_file_path("snapshot-initial");
    let rotated_url = sqlite_file_path("snapshot-rotated");
    let (url, _registry) = live_url("db.url", &initial_url);
    let pool = DbPool::<SqliteConnection>::connect(url).await.unwrap();

    let (initial, initial_generation) = pool.snapshot();
    assert_eq!(initial_generation, 0);
    assert_eq!(pool.generation(), 0);
    sql_query("CREATE TABLE only_in_generation_zero(id INTEGER PRIMARY KEY)")
        .execute(&mut initial.get().unwrap())
        .unwrap();

    pool.rotate_to(rotated_url.as_str()).await.unwrap();

    let (rotated, rotated_generation) = pool.snapshot();
    assert_eq!(rotated_generation, 1);
    assert_eq!(pool.generation(), 1);
    // Same read, so `current()` must agree with the snapshot's pool: the
    // generation-1 database is a different file and has no such table.
    assert_eq!(table_count(&rotated, "only_in_generation_zero"), 0);
    assert_eq!(table_count(&pool.current(), "only_in_generation_zero"), 0);

    // Unlike SQLx, rotation only drops the facade's handle to the r2d2 pool it
    // replaced: a handle taken before the swap keeps working, which is why the
    // Diesel backend needs no closed-pool retry.
    assert_eq!(table_count(&initial, "only_in_generation_zero"), 1);

    cleanup_sqlite_file(&initial_url);
    cleanup_sqlite_file(&rotated_url);
}

fn table_count(pool: &Pool<ConnectionManager<SqliteConnection>>, table: &str) -> i64 {
    let mut connection = pool.get().unwrap();
    sql_query(format!(
        "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' AND name = '{table}'"
    ))
    .get_result::<Count>(&mut connection)
    .unwrap()
    .count
}
