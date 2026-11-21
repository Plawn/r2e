//! Rollback-only pools: one connection inside a transaction nothing commits.

// diesel's `QueryableByName` derive expands to `field: field` with our spans.
#![allow(clippy::redundant_field_names)]

use diesel::{sql_query, Connection, QueryableByName, RunQueryDsl, SqliteConnection};
use r2e_core::http::StatusCode;
use r2e_core::{AppBuilder, ManagedContext, ManagedGuard, ManagedOutcome};
use r2e_data_diesel::{rollback_only_pool, DbPool, DbTx};

use crate::support::{cleanup_sqlite_file, sqlite_file_path};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

/// A database file with a committed `items` table.
fn migrated(label: &str) -> String {
    let path = sqlite_file_path(label);
    let mut connection = SqliteConnection::establish(&path).unwrap();
    sql_query("CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&mut connection)
        .unwrap();
    path
}

fn count(connection: &mut SqliteConnection) -> i64 {
    sql_query("SELECT COUNT(*) AS count FROM items")
        .get_result::<Count>(connection)
        .unwrap()
        .count
}

/// What an ordinary connection sees: what was committed.
fn committed(path: &str) -> i64 {
    count(&mut SqliteConnection::establish(path).unwrap())
}

#[r2e_core::test]
async fn writes_are_seen_through_the_pool_and_never_committed() {
    let path = migrated("rollback-plain");
    let pool = rollback_only_pool::<SqliteConnection>(path.as_str())
        .await
        .unwrap();

    sql_query("INSERT INTO items(name) VALUES ('a')")
        .execute(&mut pool.get().unwrap())
        .unwrap();
    // Checked out again: the same connection, still in its transaction.
    assert_eq!(count(&mut pool.get().unwrap()), 1);
    assert_eq!(committed(&path), 0);

    drop(pool);
    assert_eq!(committed(&path), 0);
    cleanup_sqlite_file(&path);
}

#[r2e_core::test]
async fn managed_transactions_of_the_app_are_savepoints() {
    let path = migrated("rollback-managed");
    let pool = DbPool::<SqliteConnection>::rollback_only(path.as_str())
        .await
        .unwrap();
    let app = AppBuilder::new().provide(pool.clone()).build_state().await;

    for (name, status) in [
        ("kept", StatusCode::CREATED),
        ("undone", StatusCode::BAD_REQUEST),
    ] {
        let mut guard = ManagedGuard::<DbTx<SqliteConnection>, _>::acquire(ManagedContext::new(
            app.state(),
            "Test",
            name,
        ))
        .await
        .unwrap();
        guard
            .resource_mut()
            .run(move |connection| {
                sql_query(format!("INSERT INTO items(name) VALUES ('{name}')")).execute(connection)
            })
            .await
            .unwrap();
        guard
            .finalize(&ManagedOutcome::from_status(status))
            .await
            .unwrap();
    }

    // The app's commit held, its rollback undid only itself…
    assert_eq!(count(&mut pool.current().get().unwrap()), 1);
    // …and neither reached the database.
    assert_eq!(committed(&path), 0);
    cleanup_sqlite_file(&path);
}
