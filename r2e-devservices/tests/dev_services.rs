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

#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires Docker"]
async fn redis_dev_service_starts_and_listens() {
    let redis = r2e_devservices::DevRedis::shared().await;
    assert!(redis.url().starts_with("redis://"));
    assert_reachable(redis.url());
}
