//! The two SPIs the app implements: **who is this request's tenant** and **how
//! is a tenant's resource built**.
//!
//! Three sources live here, and each one demonstrates a different property of
//! the layer:
//!
//! | Source | Resource | Demonstrates |
//! |---|---|---|
//! | `PoolSource` (framework) | `Pool<Sqlite>` | database-per-tenant, wired in `app.rs` |
//! | [`ApiClients`] | [`ApiClient`] | **cascade** — built on the same tenant's pool |
//! | [`Brandings`] | [`Branding`] | **fallback** — `Ok(None)` serves the app-scoped bean |

use r2e::prelude::*;
use r2e::tenant::{BoxError, BoxFuture, SyncTenantResolver, TenantContext, TenantId, TenantSource};
use serde::Serialize;
use sqlx::{Pool, Sqlite};

use crate::directory::MasterDb;

/// The header this deployment names its tenant in.
pub const TENANT_HEADER: &str = "x-tenant-id";

// ── SPI #1: request → tenant ────────────────────────────────────────────────

/// The app's [`TenantResolver`](r2e::tenant::TenantResolver): read `x-tenant-id`.
///
/// Written out rather than using the built-in `HeaderTenantResolver` because
/// the resolver is the piece every app ends up customising — a subdomain, a JWT
/// claim parked in the request extensions, a gateway header. Implementing
/// [`SyncTenantResolver`] is enough whenever resolution needs no `.await`; a
/// blanket impl bridges it to `TenantResolver`.
///
/// Note what this does **not** decide: `Ok(None)` means "this request carries no
/// tenant", and what happens then is `tenancy.on-missing`'s call (400 here).
/// A *malformed* tenant is the resolver's own call, and it is a 400 too.
#[derive(Debug, Clone, Default)]
pub struct HeaderResolver;

impl SyncTenantResolver for HeaderResolver {
    fn resolve_sync(&self, req: &RequestHead<'_>) -> Result<Option<TenantId>, HttpError> {
        match req.header(TENANT_HEADER) {
            None => Ok(None),
            Some(raw) => TenantId::parse(raw).map(Some).map_err(|error| {
                HttpError::bad_request(format!("invalid `{TENANT_HEADER}` header: {error}"))
            }),
        }
    }
}

// ── SPI #2a: the cascade — a client built on the tenant's own pool ──────────

/// A per-tenant API client: the tenant's credentials **plus** the tenant's
/// database handle.
///
/// The point of the type is that it cannot be built without the tenant's pool,
/// which is exactly the dependency the cascade resolves.
#[derive(Clone)]
pub struct ApiClient {
    tenant: TenantId,
    token: String,
    db: Pool<Sqlite>,
}

impl ApiClient {
    /// The token this client authenticates with (truncated in responses).
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Which tenant this client belongs to.
    #[must_use]
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// A query through the pool the cascade handed us — proof that the client
    /// really is holding *this* tenant's database.
    pub async fn note_count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM notes")
            .fetch_one(&self.db)
            .await
    }
}

/// [`TenantSource<ApiClient>`] — the cascade demo.
#[derive(Debug, Clone, Default)]
pub struct ApiClients;

impl TenantSource<ApiClient> for ApiClients {
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<ApiClient>, BoxError>> {
        Box::pin(async move {
            // ─── THE CASCADE ───
            // `ctx.get::<U>()` resolves **U for this same tenant**, through
            // U's own source. On the first request for a tenant this line is
            // what creates that tenant's `Pool<Sqlite>` (via `PoolSource`,
            // which queries the master directory for the DSN and connects);
            // afterwards it is a cache hit. Concurrent first requests share one
            // creation, and a cycle (A needs B needs A) is reported as
            // `TenantError::Cycle` naming the chain.
            //
            // So the resolution order here is: pool first, then this client on
            // top of it — and neither source knows how the other is wired.
            let db = ctx.get::<Pool<Sqlite>>().await?;

            // `ctx.bean::<U>()` is the other lookup: a plain **app-scoped**
            // bean out of the graph, not a per-tenant one.
            let master = ctx
                .bean::<MasterDb>()
                .ok_or("the MasterDb bean is not provided")?;

            let Some(token) = master.api_token(tenant.as_str()).await? else {
                // Unknown tenant. Unreachable in practice — the pool above
                // would already have said so — but the contract stands on its
                // own: `Ok(None)` is 404, never a made-up client.
                return Ok(None);
            };

            Ok(Some(ApiClient {
                tenant: tenant.clone(),
                token,
                db,
            }))
        })
    }
}

// ── SPI #2b: the fallback — a resource not every tenant has ────────────────

/// Per-tenant branding, with a shared default for tenants that never bought
/// custom branding.
#[derive(Debug, Clone, Serialize)]
pub struct Branding {
    /// The theme name.
    pub theme: String,
    /// Where this tenant's users are told to write.
    pub support_email: String,
}

impl Branding {
    /// The app-scoped default bean — what `.fallback_to_default()` falls back
    /// to. It is never disposed and never cached per tenant.
    #[must_use]
    pub fn shared() -> Self {
        Self {
            theme: "r2e-default".to_string(),
            support_email: "support@example.com".to_string(),
        }
    }
}

/// [`TenantSource<Branding>`] — the fallback demo.
///
/// Returns `Ok(None)` for a tenant with no `theme` row value, which with
/// `.fallback_to_default()` means "serve the app-scoped `Branding` bean"
/// instead of the 404 the strict configuration would produce.
#[derive(Debug, Clone, Default)]
pub struct Brandings;

impl TenantSource<Branding> for Brandings {
    fn create<'a>(
        &'a self,
        tenant: &'a TenantId,
        ctx: &'a TenantContext<'a>,
    ) -> BoxFuture<'a, Result<Option<Branding>, BoxError>> {
        Box::pin(async move {
            let master = ctx
                .bean::<MasterDb>()
                .ok_or("the MasterDb bean is not provided")?;

            let Some(theme) = master.theme(tenant.as_str()).await? else {
                // No custom branding (or no such tenant): fall back.
                return Ok(None);
            };

            Ok(Some(Branding {
                theme,
                support_email: format!("support@{tenant}.example"),
            }))
        })
    }
}
