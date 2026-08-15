//! Dev-service container smoke tests.
//!
//! These require a running Docker daemon and are `#[ignore]`d by default:
//!
//! ```bash
//! cargo test -p r2e-devservices --features postgres,redis --test dev_services -- --ignored
//! ```

use std::net::TcpStream;
use std::time::Duration;

/// Assert the URL's host:port accepts TCP connections.
fn assert_reachable(url: &str) {
    let hostport = url
        .rsplit('@')
        .next()
        .unwrap()
        .trim_start_matches("redis://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let addr = hostport
        .replace("localhost", "127.0.0.1")
        .parse()
        .unwrap_or_else(|e| panic!("unparseable addr {hostport}: {e}"));
    TcpStream::connect_timeout(&addr, Duration::from_secs(5))
        .unwrap_or_else(|e| panic!("cannot connect to {hostport}: {e}"));
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires Docker"]
async fn postgres_dev_service_starts_and_listens() {
    let pg = r2e_devservices::DevPostgres::shared().await;
    assert!(pg.url().starts_with("postgres://postgres:postgres@"));
    assert_reachable(pg.url());

    // shared() returns the same container on subsequent calls.
    let again = r2e_devservices::DevPostgres::shared().await;
    assert_eq!(pg.url(), again.url());
}

/// A non-default image (`pgvector/pgvector`) is usable, extension included.
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires Docker"]
async fn postgres_dev_service_runs_a_custom_image() {
    use r2e_devservices::{DevPostgres, PostgresImage};

    let pg = DevPostgres::shared_with_image(PostgresImage::new("pgvector/pgvector", "pg18")).await;
    assert_reachable(pg.url());

    let pool = sqlx::PgPool::connect(pg.url())
        .await
        .expect("cannot connect to the pgvector dev service");
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(&pool)
        .await
        .expect("the vector extension is not available in this image");

    let installed: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')")
            .fetch_one(&pool)
            .await
            .expect("cannot read pg_extension");
    assert!(installed, "vector extension missing after CREATE EXTENSION");
}

/// One shared container per image: same image reuses, different images don't.
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires Docker"]
async fn postgres_dev_service_shares_one_container_per_image() {
    use r2e_devservices::{DevPostgres, PostgresImage};

    let default = DevPostgres::shared().await;
    let default_again = DevPostgres::shared_with_image(PostgresImage::default()).await;
    assert_eq!(
        default.url(),
        default_again.url(),
        "the default image must not start a second container"
    );

    let vector =
        DevPostgres::shared_with_image(PostgresImage::new("pgvector/pgvector", "pg18")).await;
    let vector_again =
        DevPostgres::shared_with_image(PostgresImage::new("pgvector/pgvector", "pg18")).await;
    assert_eq!(
        vector.url(),
        vector_again.url(),
        "the same image must be reused"
    );
    assert_ne!(
        default.url(),
        vector.url(),
        "two images must yield two distinct containers"
    );
}

#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires Docker"]
async fn redis_dev_service_starts_and_listens() {
    let redis = r2e_devservices::DevRedis::shared().await;
    assert!(redis.url().starts_with("redis://"));
    assert_reachable(redis.url());
}
