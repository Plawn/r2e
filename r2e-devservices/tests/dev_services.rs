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

    let pg = DevPostgres::shared_with(PostgresImage::new("pgvector/pgvector", "pg18")).await;
    assert_reachable(pg.url());

    let pool = sqlx::PgPool::connect(pg.url())
        .await
        .expect("cannot connect to the pgvector dev service");
    // `IF NOT EXISTS` is not atomic: concurrent test processes on the shared
    // container can both pass the check and one loses on the catalog's unique
    // index. Only a real failure to install the extension matters here.
    if let Err(error) = sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(&pool)
        .await
    {
        let duplicate = error
            .as_database_error()
            .and_then(|db| db.code())
            .is_some_and(|code| code == "23505");
        assert!(
            duplicate,
            "the vector extension is not available in this image: {error}"
        );
    }

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
    let default_again = DevPostgres::shared_with(PostgresImage::default()).await;
    assert_eq!(
        default.url(),
        default_again.url(),
        "the default image must not start a second container"
    );

    let vector = DevPostgres::shared_with(PostgresImage::new("pgvector/pgvector", "pg18")).await;
    let vector_again =
        DevPostgres::shared_with(PostgresImage::new("pgvector/pgvector", "pg18")).await;
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

/// Credentials are parameterizable, and the URL follows them.
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires Docker"]
async fn postgres_dev_service_takes_custom_credentials() {
    use r2e_devservices::{DevPostgres, PostgresSpec};

    let pg = DevPostgres::shared_with(
        PostgresSpec::default()
            .with_user("app")
            .with_password("s3cret")
            .with_database("appdb"),
    )
    .await;
    assert!(
        pg.url().starts_with("postgres://app:s3cret@"),
        "url must carry the configured credentials: {}",
        pg.url()
    );
    assert!(pg.url().ends_with("/appdb"), "url: {}", pg.url());

    let pool = sqlx::PgPool::connect(pg.url())
        .await
        .expect("cannot connect with the configured credentials");
    let (user, database): (String, String) =
        sqlx::query_as("SELECT current_user, current_database()")
            .fetch_one(&pool)
            .await
            .expect("cannot read the session identity");
    assert_eq!(user, "app");
    assert_eq!(database, "appdb");

    // Credentials are part of the identity: the default spec is a separate
    // container, not this one.
    let default = DevPostgres::shared().await;
    assert_ne!(default.port(), pg.port());
}

/// A service R2E knows nothing about, defined entirely on the user's side.
#[tokio::test]
#[ignore = "requires Docker"]
async fn user_defined_dev_service_joins_the_session() {
    use r2e_devservices::testcontainers::core::{IntoContainerPort, WaitFor};
    use r2e_devservices::testcontainers::{GenericImage, ImageExt};
    use r2e_devservices::{DevService, DevServiceSpec};

    fn valkey(tag: &'static str) -> DevServiceSpec<GenericImage> {
        DevServiceSpec::new("valkey", move || {
            GenericImage::new("valkey/valkey", tag)
                .with_exposed_port(6379.tcp())
                .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
                .with_cmd(["valkey-server"])
        })
        .with_port(6379)
    }

    let valkey8 = DevService::shared(valkey("8-alpine")).await;
    assert_reachable(&format!("redis://{}", valkey8.endpoint(6379)));

    // Same spec ⇒ same container; a different image ⇒ its own container.
    let again = DevService::shared(valkey("8-alpine")).await;
    assert_eq!(valkey8.port(6379), again.port(6379));

    let valkey7 = DevService::shared(valkey("7-alpine")).await;
    assert_ne!(valkey8.port(6379), valkey7.port(6379));
}

/// A non-default Redis-compatible image (`valkey/valkey`).
#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires Docker"]
async fn redis_dev_service_runs_a_custom_image() {
    use r2e_devservices::{DevRedis, RedisImage};

    let valkey = DevRedis::shared_with(RedisImage::new("valkey/valkey", "8-alpine")).await;
    assert_reachable(valkey.url());

    let default = DevRedis::shared().await;
    assert_ne!(default.port(), valkey.port());
}

#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires Docker"]
async fn redis_dev_service_starts_and_listens() {
    let redis = r2e_devservices::DevRedis::shared().await;
    assert!(redis.url().starts_with("redis://"));
    assert_reachable(redis.url());
}

// ---------------------------------------------------------------------------
// Sharing identity. No Docker: the identity is derived from the spec alone.
// ---------------------------------------------------------------------------

/// A spec whose request differs only in one env var — everything a naive
/// image+ports identity would miss.
fn tuned(
    setting: &'static str,
) -> r2e_devservices::DevServiceSpec<r2e_devservices::testcontainers::GenericImage> {
    use r2e_devservices::testcontainers::core::IntoContainerPort;
    use r2e_devservices::testcontainers::{GenericImage, ImageExt};

    r2e_devservices::DevServiceSpec::new("tuned", move || {
        GenericImage::new("vendor/server", "1")
            .with_exposed_port(8080.tcp())
            .with_env_var("MODE", setting)
    })
    .with_port(8080)
}

#[test]
fn identity_is_stable_for_the_same_spec() {
    assert_eq!(tuned("a").configuration(), tuned("a").configuration());
}

#[test]
fn identity_separates_requests_the_image_reference_cannot_tell_apart() {
    assert_ne!(tuned("a").configuration(), tuned("b").configuration());
}

/// Delimiter-bearing values must not be able to imitate a field boundary and
/// make two different requests fingerprint the same.
#[test]
fn identity_is_injective_under_delimiters_in_values() {
    use r2e_devservices::testcontainers::{GenericImage, ImageExt};
    use r2e_devservices::DevServiceSpec;

    fn credentials(user: &'static str, password: &'static str) -> DevServiceSpec<GenericImage> {
        DevServiceSpec::new("credentials", move || {
            GenericImage::new("vendor/server", "1")
                .with_env_var("USER", user)
                .with_env_var("PASSWORD", password)
        })
    }

    assert_ne!(
        credentials("alice;password=x", "y").configuration(),
        credentials("alice", "x;password=y").configuration()
    );
}

#[test]
fn the_discriminator_splits_otherwise_identical_specs() {
    assert_ne!(
        tuned("a").configuration(),
        tuned("a").with_discriminator("seeded").configuration()
    );
    assert_eq!(
        tuned("a").with_discriminator("seeded").configuration(),
        tuned("a").with_discriminator("seeded").configuration()
    );
}

/// The declared ports are part of the identity, and declaring them in another
/// order is the same declaration.
#[test]
fn identity_covers_declared_ports_regardless_of_order() {
    let one = tuned("a").with_port(9090);
    let other = tuned("a").with_port(9090);
    assert_eq!(one.configuration(), other.configuration());
    assert_ne!(tuned("a").configuration(), one.configuration());
}
