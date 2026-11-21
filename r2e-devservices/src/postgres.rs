use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use crate::service::{DevService, DevServiceSpec};

/// Default image repository.
const DEFAULT_NAME: &str = "postgres";
/// Default image tag. The testcontainers module's own default
/// (`postgres:11-alpine`) predates arm64 images and fails with
/// `exec format error` on Apple Silicon.
const DEFAULT_TAG: &str = "16-alpine";
const CONTAINER_PORT: u16 = 5432;

/// The Docker image backing a [`DevPostgres`] container.
///
/// The default is `postgres:16-alpine`. Use [`new`](Self::new) to run a
/// Postgres distribution that ships extra extensions:
///
/// ```ignore
/// let pg = DevPostgres::shared_with(PostgresImage::new("pgvector/pgvector", "pg18")).await;
/// ```
///
/// Any reference Docker accepts works, private registries included
/// (`registry.example.com:5000/team/pg`), as does a locally built image that
/// was never pushed — the container is created first and only pulled if Docker
/// reports it missing.
///
/// The image is part of the shared container's identity, so two different
/// images yield two distinct shared containers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostgresImage {
    name: String,
    tag: String,
}

impl Default for PostgresImage {
    /// `postgres:16-alpine`.
    fn default() -> Self {
        Self {
            name: DEFAULT_NAME.to_string(),
            tag: DEFAULT_TAG.to_string(),
        }
    }
}

impl PostgresImage {
    /// An image by repository and tag, e.g. `("pgvector/pgvector", "pg18")`.
    ///
    /// The image must behave like the official `postgres` one: it must honour
    /// `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`, listen on 5432, and
    /// log `database system is ready to accept connections` when up. Anything
    /// further from the official image belongs in a
    /// [`DevService`](crate::DevService) of its own.
    pub fn new(name: impl Into<String>, tag: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tag: tag.into(),
        }
    }

    /// The default `postgres` repository with a specific tag.
    pub fn with_tag(tag: impl Into<String>) -> Self {
        Self::new(DEFAULT_NAME, tag)
    }

    /// The image repository, e.g. `pgvector/pgvector`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The image tag, e.g. `pg18`.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// The full image reference, `{name}:{tag}`.
    pub fn reference(&self) -> String {
        format!("{}:{}", self.name, self.tag)
    }
}

/// Everything that defines a [`DevPostgres`] container: the image and the
/// credentials it is initialized with.
///
/// The whole spec is part of the shared container's identity — changing the
/// image *or* the credentials yields a separate shared container.
///
/// ```ignore
/// let pg = DevPostgres::shared_with(
///     PostgresSpec::new(PostgresImage::new("pgvector/pgvector", "pg18"))
///         .with_user("app")
///         .with_database("appdb"),
/// )
/// .await;
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostgresSpec {
    image: PostgresImage,
    user: String,
    password: String,
    database: String,
}

impl Default for PostgresSpec {
    /// `postgres:16-alpine` with the `postgres`/`postgres`/`postgres`
    /// credentials.
    fn default() -> Self {
        Self::new(PostgresImage::default())
    }
}

impl From<PostgresImage> for PostgresSpec {
    fn from(image: PostgresImage) -> Self {
        Self::new(image)
    }
}

impl PostgresSpec {
    /// An image with the default `postgres`/`postgres`/`postgres` credentials.
    pub fn new(image: PostgresImage) -> Self {
        Self {
            image,
            user: "postgres".to_string(),
            password: "postgres".to_string(),
            database: "postgres".to_string(),
        }
    }

    /// The superuser to create (`POSTGRES_USER`).
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = user.into();
        self
    }

    /// The password to set (`POSTGRES_PASSWORD`).
    ///
    /// Any password works: it reaches the container verbatim and is
    /// percent-encoded in [`url`](DevPostgres::url).
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = password.into();
        self
    }

    /// The database to create (`POSTGRES_DB`).
    pub fn with_database(mut self, database: impl Into<String>) -> Self {
        self.database = database.into();
        self
    }

    /// The image.
    pub fn image(&self) -> &PostgresImage {
        &self.image
    }

    fn service_spec(&self) -> DevServiceSpec<Postgres> {
        let Self {
            image,
            user,
            password,
            database,
        } = self.clone();
        // No explicit discriminator: the credentials reach the container as
        // `POSTGRES_*` env vars, which the derived identity already covers.
        DevServiceSpec::new("postgres", move || {
            Postgres::default()
                .with_user(&user)
                .with_password(&password)
                .with_db_name(&database)
                .with_name(&image.name)
                .with_tag(&image.tag)
        })
        .with_port(CONTAINER_PORT)
    }
}

/// A containerized PostgreSQL instance for tests.
///
/// Credentials default to `postgres`/`postgres`, database `postgres`; override
/// them through [`PostgresSpec`].
pub struct DevPostgres {
    container: Container,
    user: String,
    password: String,
    host: String,
    port: u16,
    url: String,
}

/// The container behind a handle.
enum Container {
    /// An isolated container, alive as long as one handle onto it is — the
    /// handles [`fresh_database`](DevPostgres::fresh_database) derives share
    /// the ownership, so the first one can be dropped.
    Owned(Arc<DevService>),
    /// The shared container: it belongs to the process-wide registry and
    /// outlives every handle, so dropping a handle stops nothing.
    Shared(&'static DevService),
}

impl Container {
    fn service(&self) -> &DevService {
        match self {
            Self::Owned(service) => service,
            Self::Shared(service) => service,
        }
    }

    fn share(&self) -> Self {
        match self {
            Self::Owned(service) => Self::Owned(Arc::clone(service)),
            Self::Shared(service) => Self::Shared(service),
        }
    }
}

impl DevPostgres {
    /// Start a fresh, isolated PostgreSQL container (`postgres:16-alpine`).
    ///
    /// # Panics
    ///
    /// Panics if Docker is unavailable or the container fails to start.
    pub async fn start() -> Self {
        Self::start_with(PostgresSpec::default()).await
    }

    /// [`start`](Self::start) with a specific `postgres` image tag.
    pub async fn start_with_tag(tag: &str) -> Self {
        Self::start_with(PostgresImage::with_tag(tag)).await
    }

    /// [`start`](Self::start) for a [`PostgresImage`] or a full
    /// [`PostgresSpec`].
    pub async fn start_with(spec: impl Into<PostgresSpec>) -> Self {
        let spec = spec.into();
        let service = DevService::start(spec.service_spec()).await;
        Self::describe(Container::Owned(Arc::new(service)), &spec)
    }

    /// The cross-process shared PostgreSQL container, started on first use.
    ///
    /// Tests sharing the container must not assume an empty database — take a
    /// [`fresh_database`](Self::fresh_database) per test, or use
    /// [`start`](Self::start) for an isolated container.
    ///
    /// The *container* is shared; the returned handle is a cheap owned copy of
    /// its connection details, so dropping it stops nothing.
    pub async fn shared() -> Self {
        Self::shared_with(PostgresSpec::default()).await
    }

    /// [`shared`](Self::shared) for a [`PostgresImage`] or a full
    /// [`PostgresSpec`].
    ///
    /// One shared container per spec: same spec ⇒ same container, a different
    /// image or different credentials ⇒ a separate one, within the process and
    /// across the test binaries of the session.
    pub async fn shared_with(spec: impl Into<PostgresSpec>) -> Self {
        let spec = spec.into();
        let service = DevService::shared(spec.service_spec()).await;
        Self::describe(Container::Shared(service), &spec)
    }

    /// The connection details of a running container.
    fn describe(container: Container, spec: &PostgresSpec) -> Self {
        let host = container.service().host().to_string();
        let port = container.service().port(CONTAINER_PORT);
        Self {
            url: database_url(&spec.user, &spec.password, &host, port, &spec.database),
            container,
            user: spec.user.clone(),
            password: spec.password.clone(),
            host,
            port,
        }
    }

    /// Create a new, empty database on this container and return a handle
    /// whose [`url`](Self::url) points at it.
    ///
    /// This is how tests share one container without sharing state: the
    /// container starts once per session, each test gets a database of its own.
    ///
    /// ```ignore
    /// let db = DevPostgres::shared().await.fresh_database().await;
    /// let app = TestApp::boot_with::<MyApp>(|b| {
    ///     b.override_config_value("datasource.url", db.url())
    /// })
    /// .await;
    /// ```
    ///
    /// The database is created with `createdb` inside the container, so the
    /// test needs no Postgres client of its own. Its name is unique across the
    /// test processes of the session. It is not dropped afterwards: it goes
    /// away with the container.
    ///
    /// # Panics
    ///
    /// Panics if the database cannot be created.
    pub async fn fresh_database(&self) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        // The process id separates test binaries, the counter separates tests,
        // and the clock separates this run from an earlier one whose container
        // (and process id) is being reused.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let database = format!(
            "r2e_{}_{}_{nanos:x}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );

        // Over the container's local socket, which the official image trusts.
        let output = self
            .container
            .service()
            .exec(["createdb", "-U", self.user.as_str(), database.as_str()])
            .await;
        assert!(
            output.success(),
            "cannot create the `{database}` database on the dev PostgreSQL container \
             (createdb exited with {}): {}",
            output.exit_code,
            output.stderr.trim()
        );

        Self {
            container: self.container.share(),
            url: database_url(&self.user, &self.password, &self.host, self.port, &database),
            user: self.user.clone(),
            password: self.password.clone(),
            host: self.host.clone(),
            port: self.port,
        }
    }

    /// Connection URL: `postgres://{user}:{password}@{host}:{port}/{database}`,
    /// with the credentials percent-encoded so any value round-trips.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The host the container is reachable on.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The host port PostgreSQL is published on.
    pub fn port(&self) -> u16 {
        self.port
    }
}

fn database_url(user: &str, password: &str, host: &str, port: u16, database: &str) -> String {
    format!(
        "postgres://{}:{}@{host}:{port}/{}",
        encoded(user),
        encoded(password),
        encoded(database)
    )
}

/// Percent-encode a credential for the connection URL.
///
/// Everything outside RFC 3986's unreserved set is escaped, so a password
/// holding `@`, `/` or `:` — legal for Postgres — still yields a URL that
/// parses back to the value the container was initialized with.
fn encoded(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
