//! Rollback-only pools: one connection, one transaction nothing commits.

use r2e_data_sqlx::{rollback_only_pool, DbPool};
use sqlx::{Connection, Executor, Row, Sqlite, SqliteConnection};

use crate::support::{cleanup_sqlite_file, sqlite_file_url};

/// A database file holding an empty `items` table, created on a connection of
/// its own — the way a test database is migrated before the tests run.
async fn migrated(label: &str) -> String {
    let url = sqlite_file_url(label);
    let mut connection = SqliteConnection::connect(&url).await.unwrap();
    connection
        .execute("CREATE TABLE items(name TEXT NOT NULL UNIQUE)")
        .await
        .unwrap();
    connection.close().await.unwrap();
    url
}

/// What a connection that is not the pool's sees.
async fn committed_items(url: &str) -> i64 {
    let mut connection = SqliteConnection::connect(url).await.unwrap();
    let count = sqlx::query("SELECT COUNT(*) FROM items")
        .fetch_one(&mut connection)
        .await
        .unwrap()
        .get(0);
    connection.close().await.unwrap();
    count
}

async fn items(pool: &DbPool<Sqlite>) -> i64 {
    sqlx::query("SELECT COUNT(*) FROM items")
        .fetch_one(pool)
        .await
        .unwrap()
        .get(0)
}

#[r2e_core::test]
async fn writes_are_seen_through_the_pool_and_never_committed() {
    let url = migrated("rollback-direct").await;
    let pool = DbPool::<Sqlite>::rollback_only(url.as_str()).await.unwrap();

    (&pool)
        .execute("INSERT INTO items(name) VALUES ('a')")
        .await
        .unwrap();

    assert_eq!(items(&pool).await, 1);
    assert_eq!(committed_items(&url).await, 0);

    pool.current().close().await;
    assert_eq!(committed_items(&url).await, 0);
    cleanup_sqlite_file(&url);
}

#[r2e_core::test]
async fn a_committed_transaction_of_the_app_is_a_savepoint() {
    let url = migrated("rollback-commit").await;
    let pool = DbPool::<Sqlite>::rollback_only(url.as_str()).await.unwrap();

    let (mut transaction, generation) = pool.begin().await.unwrap();
    assert_eq!(generation, 0);
    transaction
        .execute("INSERT INTO items(name) VALUES ('a')")
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    // Committed as far as the app can tell, and still not in the database.
    assert_eq!(items(&pool).await, 1);
    assert_eq!(committed_items(&url).await, 0);

    pool.current().close().await;
    assert_eq!(committed_items(&url).await, 0);
    cleanup_sqlite_file(&url);
}

#[r2e_core::test]
async fn a_rolled_back_transaction_of_the_app_undoes_only_itself() {
    let url = migrated("rollback-rollback").await;
    let pool = DbPool::<Sqlite>::rollback_only(url.as_str()).await.unwrap();

    (&pool)
        .execute("INSERT INTO items(name) VALUES ('kept')")
        .await
        .unwrap();

    let (mut transaction, _) = pool.begin().await.unwrap();
    transaction
        .execute("INSERT INTO items(name) VALUES ('undone')")
        .await
        .unwrap();
    transaction.rollback().await.unwrap();

    let (mut dropped, _) = pool.begin().await.unwrap();
    dropped
        .execute("INSERT INTO items(name) VALUES ('dropped')")
        .await
        .unwrap();
    drop(dropped);

    let names: Vec<String> = sqlx::query("SELECT name FROM items ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(names, ["kept"]);

    pool.current().close().await;
    cleanup_sqlite_file(&url);
}

#[r2e_core::test]
async fn two_pools_on_one_database_do_not_see_each_other() {
    let url = migrated("rollback-isolated").await;

    let first = DbPool::<Sqlite>::rollback_only(url.as_str()).await.unwrap();
    (&first)
        .execute("INSERT INTO items(name) VALUES ('a')")
        .await
        .unwrap();
    first.current().close().await;

    // The same row again: the first pool's insert is gone, not a conflict.
    let second = DbPool::<Sqlite>::rollback_only(url.as_str()).await.unwrap();
    assert_eq!(items(&second).await, 0);
    (&second)
        .execute("INSERT INTO items(name) VALUES ('a')")
        .await
        .unwrap();

    second.current().close().await;
    cleanup_sqlite_file(&url);
}

#[r2e_core::test]
async fn a_plain_pool_is_rollback_only_too() {
    let url = migrated("rollback-plain").await;
    let pool = rollback_only_pool::<Sqlite>(&url).await.unwrap();

    let mut transaction = pool.begin().await.unwrap();
    transaction
        .execute("INSERT INTO items(name) VALUES ('a')")
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    let count: i64 = sqlx::query("SELECT COUNT(*) FROM items")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);
    assert_eq!(committed_items(&url).await, 0);

    pool.close().await;
    assert_eq!(committed_items(&url).await, 0);
    cleanup_sqlite_file(&url);
}
