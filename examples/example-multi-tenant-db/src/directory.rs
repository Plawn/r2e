//! The **master directory**: the one table that says which tenants exist.
//!
//! Nothing in R2E knows about tenants — the framework routes to whatever the
//! app's directory says. Here that directory is a SQLite `master.db` holding
//! one row per tenant:
//!
//! ```text
//! tenants(slug, dsn, api_token, theme)
//! ```
//!
//! - `dsn` is the tenant's **own database file**, which is what makes the
//!   isolation in this example physical rather than a `WHERE tenant_id = ?`
//!   convention (that model is `examples/example-multi-tenant`).
//! - `api_token` feeds the cascade demo (a per-tenant client built on the
//!   per-tenant pool).
//! - `theme` is nullable, and that null is the fallback demo: a tenant without
//!   custom branding is served the app-scoped default bean.
//!
//! In a real deployment this is a Postgres table, a control-plane API, or a
//! config service. The shape the framework cares about is the same: one lookup
//! with three answers — provisioned, unknown, or "the directory itself is
//! down".

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// The seeded tenants: slug, API token, optional custom theme, initial notes.
const SEED: &[(&str, &str, Option<&str>, &[&str])] = &[
    (
        "acme",
        "acme-token-7f3",
        Some("acme-dark"),
        &["Ship the beta", "Order more anvils"],
    ),
    // No theme → `Brandings` returns `Ok(None)` → the app-scoped default.
    (
        "globex",
        "globex-token-22a",
        None,
        &["Acquire a smaller company"],
    ),
];

/// The master database, as an app-scoped bean.
///
/// A newtype rather than a bare `SqlitePool`: the per-tenant pools live in
/// `Tenanted<Pool<Sqlite>>`, and naming the master one keeps "which pool is
/// this?" answerable at a glance.
#[derive(Clone)]
pub struct MasterDb(SqlitePool);

/// One row of the directory, as served by `GET /admin/tenants`.
#[derive(Debug, Clone, Serialize)]
pub struct TenantRecord {
    /// The tenant id clients send in `x-tenant-id`.
    pub slug: String,
    /// Where that tenant's data lives.
    pub dsn: String,
    /// `None` = no custom branding, i.e. the fallback bean is used.
    pub theme: Option<String>,
}

impl MasterDb {
    /// The DSN of a tenant's database, `None` when the tenant does not exist.
    ///
    /// This is the whole of `PoolSource`'s job: `Ok(None)` means 404, `Err`
    /// means the directory is unreachable (503, retried on the next request).
    pub async fn dsn(&self, slug: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT dsn FROM tenants WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.0)
            .await
    }

    /// A tenant's API token, `None` when the tenant does not exist.
    pub async fn api_token(&self, slug: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT api_token FROM tenants WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.0)
            .await
    }

    /// A tenant's custom theme. `None` covers both "unknown tenant" and "known
    /// tenant, no custom branding" — the fallback treats them the same way.
    pub async fn theme(&self, slug: &str) -> Result<Option<String>, sqlx::Error> {
        let row: Option<Option<String>> = sqlx::query_scalar("SELECT theme FROM tenants WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.0)
            .await?;
        Ok(row.flatten())
    }

    /// Every provisioned tenant.
    pub async fn list(&self) -> Result<Vec<TenantRecord>, sqlx::Error> {
        let rows: Vec<(String, String, Option<String>)> =
            sqlx::query_as("SELECT slug, dsn, theme FROM tenants ORDER BY slug")
                .fetch_all(&self.0)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(slug, dsn, theme)| TenantRecord { slug, dsn, theme })
            .collect())
    }
}

/// A fresh directory for this boot.
///
/// Unique per process **and** per boot so that the integration tests — which
/// boot the app once per test — never share seeded data.
#[must_use]
pub fn data_dir() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "r2e-multi-tenant-db-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Create the master database plus one database **per tenant**, seeded.
///
/// Stands in for a provisioning pipeline: in production a tenant's database is
/// created and migrated when the tenant signs up, never on the request path
/// (see the "per-tenant migrations" note in `docs/features/24-tenancy.md`).
pub async fn provision(dir: &Path) -> MasterDb {
    std::fs::create_dir_all(dir).expect("create the example's data directory");

    let master = connect(&file_url(dir, "master")).await;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS tenants (
            slug      TEXT PRIMARY KEY,
            dsn       TEXT NOT NULL,
            api_token TEXT NOT NULL,
            theme     TEXT
        )",
    )
    .execute(&master)
    .await
    .expect("create the tenants table");

    for (slug, token, theme, notes) in SEED {
        let dsn = file_url(dir, slug);

        // The tenant's own database, created and migrated here.
        let tenant_db = connect(&dsn).await;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS notes (
                id   INTEGER PRIMARY KEY AUTOINCREMENT,
                body TEXT NOT NULL
            )",
        )
        .execute(&tenant_db)
        .await
        .expect("create the notes table");
        for body in *notes {
            sqlx::query("INSERT INTO notes (body) VALUES (?)")
                .bind(body)
                .execute(&tenant_db)
                .await
                .expect("seed a note");
        }
        tenant_db.close().await;

        sqlx::query("INSERT OR REPLACE INTO tenants (slug, dsn, api_token, theme) VALUES (?, ?, ?, ?)")
            .bind(slug)
            .bind(&dsn)
            .bind(token)
            .bind(*theme)
            .execute(&master)
            .await
            .expect("seed a tenant");
    }

    MasterDb(master)
}

async fn connect(url: &str) -> SqlitePool {
    SqlitePoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await
        .unwrap_or_else(|error| panic!("connect to {url}: {error}"))
}

fn file_url(dir: &Path, name: &str) -> String {
    format!("sqlite://{}?mode=rwc", dir.join(format!("{name}.db")).display())
}
