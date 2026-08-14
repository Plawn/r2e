//! The four ways a route touches a tenant.
//!
//! | Controller | Shape | What it shows |
//! |---|---|---|
//! | [`NotesController`] | `#[managed] tx: &mut TenantTx<'_, Sqlite>` | a transaction on the tenant's own database |
//! | [`ApiClientController`] | `#[inject(request)] client: Tenant<ApiClient>` | the cascade, from the call site |
//! | [`BrandingController`] | `#[inject(request)] branding: Tenant<Branding>` | the fallback, from the call site |
//! | [`AdminController`] | `#[inject] pools: TenantPools<Sqlite>` | the maps as app-scoped beans |
//!
//! Note what none of them declare: which tenant. The tenant comes from the
//! request, and forgetting the `Tenancy` / `PerTenant` plugins is a **compile
//! error** at `register_controllers`, not a 500 on the first request from the
//! first tenant.

use r2e::prelude::*;
use r2e::r2e_data_sqlx::{TenantPools, TenantTx};
use r2e::tenant::{Tenant, TenantId, Tenanted};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Sqlite;

use crate::directory::{MasterDb, TenantRecord};
use crate::tenancy::{ApiClient, Branding};

fn internal(error: impl std::fmt::Display) -> HttpError {
    HttpError::internal(error.to_string())
}

// ── Notes: a managed transaction on the tenant's own database ──────────────

/// What one tenant's notes look like.
#[derive(Debug, Serialize)]
pub struct Notes {
    /// The tenant the transaction actually ran for.
    pub tenant: String,
    /// Its notes — physically in its own SQLite file.
    pub notes: Vec<String>,
}

/// A new note.
#[derive(Debug, Deserialize)]
pub struct NewNote {
    /// The note text.
    pub body: String,
}

/// Reads and writes on the requesting tenant's database.
///
/// The controller has **no fields**: `TenantTx` resolves the tenant itself
/// (reusing the request's memoized answer when another extractor already
/// resolved it), opens or reuses that tenant's pool, and begins a transaction
/// that commits on `Ok` and rolls back on `Err` — the same lifecycle as the
/// single-tenant `Tx`.
#[controller(path = "/notes")]
pub struct NotesController;

#[routes]
impl NotesController {
    /// List this tenant's notes.
    #[get("/")]
    async fn list(&self, #[managed] tx: &mut TenantTx<'_, Sqlite>) -> Result<Json<Notes>, HttpError> {
        let notes: Vec<String> = sqlx::query_scalar("SELECT body FROM notes ORDER BY id")
            .fetch_all(tx.connection())
            .await
            .map_err(internal)?;
        Ok(Json(Notes {
            tenant: tx.tenant().to_string(),
            notes,
        }))
    }

    /// Add a note. Committed on this tenant's database, invisible to the others.
    #[post("/")]
    async fn create(
        &self,
        #[managed] tx: &mut TenantTx<'_, Sqlite>,
        Json(body): Json<NewNote>,
    ) -> Result<(StatusCode, Json<Notes>), HttpError> {
        sqlx::query("INSERT INTO notes (body) VALUES (?)")
            .bind(&body.body)
            .execute(tx.connection())
            .await
            .map_err(internal)?;

        let notes: Vec<String> = sqlx::query_scalar("SELECT body FROM notes ORDER BY id")
            .fetch_all(tx.connection())
            .await
            .map_err(internal)?;

        Ok((
            StatusCode::CREATED,
            Json(Notes {
                tenant: tx.tenant().to_string(),
                notes,
            }),
        ))
    }

    /// Writes, then fails — the insert must not survive the response, and only
    /// this tenant's database is touched.
    #[post("/rollback-demo")]
    async fn rollback_demo(
        &self,
        #[managed] tx: &mut TenantTx<'_, Sqlite>,
    ) -> Result<Json<Notes>, HttpError> {
        sqlx::query("INSERT INTO notes (body) VALUES ('this will be rolled back')")
            .execute(tx.connection())
            .await
            .map_err(internal)?;
        Err(HttpError::bad_request("deliberate failure after a write"))
    }
}

// ── Who am I: the id alone, provisioning nothing ───────────────────────────

/// The cheapest tenancy field: the resolved id, with no per-tenant resource
/// touched at all.
#[controller(path = "/whoami")]
pub struct WhoAmIController {
    #[inject(request)]
    tenant: TenantId,
}

#[routes]
impl WhoAmIController {
    #[get("/")]
    async fn whoami(&self) -> Json<Value> {
        Json(json!({ "tenant": self.tenant }))
    }
}

// ── Cascade: a per-tenant client built on the per-tenant pool ──────────────

/// Serves the per-tenant [`ApiClient`], whose source resolved this same
/// tenant's `Pool<Sqlite>` before building it.
#[controller(path = "/client")]
pub struct ApiClientController {
    #[inject(request)]
    client: Tenant<ApiClient>,
}

#[routes]
impl ApiClientController {
    /// The client's identity plus a count queried *through the cascaded pool*.
    #[get("/")]
    async fn describe(&self) -> Result<Json<Value>, HttpError> {
        let notes = self.client.note_count().await.map_err(internal)?;
        Ok(Json(json!({
            "tenant": self.client.tenant(),
            // Never log or return a real credential; this is a demo token.
            "token": self.client.token(),
            "notes_visible_through_the_cascaded_pool": notes,
        })))
    }
}

// ── Fallback: a resource not every tenant has ──────────────────────────────

/// Serves per-tenant [`Branding`], falling back to the app-scoped bean.
#[controller(path = "/branding")]
pub struct BrandingController {
    #[inject(request)]
    branding: Tenant<Branding>,
}

#[routes]
impl BrandingController {
    /// `acme` has its own theme; `globex` (and any unknown tenant) gets the
    /// shared default — a 200, not the 404 `/notes` would return.
    #[get("/")]
    async fn show(&self) -> Json<Value> {
        Json(json!({
            "tenant": self.branding.tenant_id(),
            "branding": &*self.branding,
        }))
    }
}

// ── Admin: the maps are ordinary app-scoped beans ──────────────────────────

/// Operational routes over the per-tenant maps.
///
/// `Tenanted<T>` is a plain bean, so `#[inject]` reaches it from a controller
/// that has nothing to do with a tenant request. These routes take **no**
/// `x-tenant-id`.
#[controller(path = "/admin")]
pub struct AdminController {
    #[inject]
    master: MasterDb,
    #[inject]
    pools: TenantPools<Sqlite>,
    #[inject]
    clients: Tenanted<ApiClient>,
}

#[routes]
impl AdminController {
    /// The directory itself — which tenants exist, and where their data lives.
    #[get("/tenants")]
    async fn tenants(&self) -> Result<Json<Vec<TenantRecord>>, HttpError> {
        Ok(Json(self.master.list().await.map_err(internal)?))
    }

    /// Live per-tenant pools: who is warm, how idle, and the counters.
    #[get("/pools")]
    async fn pools(&self) -> Json<Value> {
        Json(map_view(&self.pools))
    }

    /// The same view for the cascaded client map.
    #[get("/clients")]
    async fn clients(&self) -> Json<Value> {
        Json(map_view(&self.clients))
    }

    /// Drop a tenant's resources **and dispose of them** — `PoolSource::dispose`
    /// closes the pool, so the connections are released now rather than
    /// whenever the last handle happens to drop.
    ///
    /// The client is evicted first: it holds a clone of the pool, and evicting
    /// it first means nothing is left pointing at the pool being closed.
    #[post("/tenants/{tenant}/evict")]
    async fn evict(&self, Path(tenant): Path<String>) -> Result<Json<Value>, HttpError> {
        let tenant = parse_tenant(&tenant)?;
        let client = self.clients.evict(&tenant).await;
        let pool = self.pools.evict(&tenant).await;
        Ok(Json(json!({ "evicted_client": client, "evicted_pool": pool })))
    }

    /// Forget a tenant's resources **without** disposing of them — the shape
    /// for "its DSN changed in the directory": the next request rebuilds from
    /// the new record while in-flight requests finish on the old pool.
    #[post("/tenants/{tenant}/invalidate")]
    async fn invalidate(&self, Path(tenant): Path<String>) -> Result<Json<Value>, HttpError> {
        let tenant = parse_tenant(&tenant)?;
        let client = self.clients.invalidate(&tenant);
        let pool = self.pools.invalidate(&tenant);
        Ok(Json(
            json!({ "invalidated_client": client, "invalidated_pool": pool }),
        ))
    }
}

fn parse_tenant(raw: &str) -> Result<TenantId, HttpError> {
    TenantId::parse(raw).map_err(|error| HttpError::bad_request(error.to_string()))
}

/// `Tenanted<T>`'s introspection, as JSON.
///
/// Hand-rolled because `TenantedMetrics` / `TenantStats` are plain data types
/// without `Serialize` — deliberate on the framework's side (no serde in the
/// bean's public surface), a few lines here.
fn map_view<T: Clone + Send + Sync + 'static>(map: &Tenanted<T>) -> Value {
    let metrics = map.metrics();
    json!({
        "active": map.active(),
        "stats": map
            .stats()
            .into_iter()
            .map(|stat| json!({
                "tenant": stat.tenant,
                "ready": stat.ready,
                "idle_ms": u64::try_from(stat.idle.as_millis()).unwrap_or(u64::MAX),
            }))
            .collect::<Vec<_>>(),
        "metrics": {
            "active": metrics.active,
            "negative": metrics.negative,
            "hits": metrics.hits,
            "created": metrics.created,
            "create_failures": metrics.create_failures,
            "timeouts": metrics.timeouts,
            "unknown": metrics.unknown,
            "fallbacks": metrics.fallbacks,
            "disposed": metrics.disposed,
            "evicted_idle": metrics.evicted_idle,
            "evicted_lru": metrics.evicted_lru,
        },
    })
}
