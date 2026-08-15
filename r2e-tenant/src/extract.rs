//! Request extractors: [`Tenant<T>`] and [`TenantId`].
//!
//! `Tenant<T>` is the whole point of the crate at the call site — a
//! request-scoped controller field that resolves the request's tenant, gets
//! **that tenant's** `T`, and derefs to it:
//!
//! ```ignore
//! #[controller(path = "/orders")]
//! struct OrderController {
//!     #[inject(request)]
//!     db: Tenant<Pool<Postgres>>,
//! }
//!
//! #[routes]
//! impl OrderController {
//!     #[get("/")]
//!     async fn list(&self) -> Result<Json<Vec<Order>>, HttpError> {
//!         let rows = sqlx::query_as("select * from orders")
//!             .fetch_all(&*self.db)
//!             .await?;
//!         Ok(Json(rows))
//!     }
//! }
//! ```
//!
//! `#[inject(request)]` is a **field** attribute: the tenancy extractors are
//! request-scoped controller fields, not handler parameters (handler parameters
//! go through axum's own `FromRequestParts`, which these types deliberately do
//! not implement — see below).
//!
//! # Why these are `FromRequestPartsVia`, not `FromRequestParts`
//!
//! Both extractors read beans out of the HList state — the
//! [`TenantRouter`](crate::TenantRouter) always, plus [`Tenanted<T>`] for
//! `Tenant<T>` — and their index witnesses cannot live on the impl (E0207), so
//! they are parked in the [`ViaBean`] marker: `ViaBean<I>` for `TenantId`,
//! `ViaBean<(I, J)>` for `Tenant<T>`. Two consequences worth knowing:
//!
//! - **Forgetting a plugin is a compile error**, not a 500: without
//!   `.plugin(Tenancy::resolver::<_>())` the state has no `TenantRouter` and
//!   `register_controllers` fails; without `.plugin(PerTenant::<T>::from::<_>())`
//!   it has no `Tenanted<T>` and the same happens.
//! - Neither type implements axum's `FromRequestParts` — a second route would
//!   make the marker ambiguous. That invariant is pinned by
//!   [`assert_unambiguous_extractor`](r2e_core::extract::assert_unambiguous_extractor)
//!   probes in `tests/tenant/extractor.rs`.
//!
//! # Optional forms
//!
//! `Option<Tenant<T>>` and `Option<TenantId>` yield `None` when the request
//! carries **no** tenant and `tenancy.on-missing = allow` (or tenancy is
//! disabled) — the shape for a route that serves both a tenant-scoped and a
//! global view. A tenant that is present but unknown or unavailable is still an
//! error: `Option` covers "no tenant", never "bad tenant".
//!
//! # Ordering: resolvers that read what authentication left behind
//!
//! An [`ExtensionTenantResolver`](crate::ExtensionTenantResolver) projects a
//! claim some earlier extractor parked in `parts.extensions` — the usual "the
//! tenant is a JWT claim" shape. That only works if the identity extractor runs
//! **before** the resolver does, and who runs first depends on where the
//! identity is declared:
//!
//! - **Struct-level identity** (`#[inject(identity)]` as a controller field) —
//!   always fine. Identity and tenancy are both request-scoped controller
//!   fields, extracted in declaration order by the same generated extractor, and
//!   the identity field is emitted first.
//! - **Parameter-level identity** (`#[inject(identity)]` on a handler
//!   parameter) — fine for handler parameters and `#[managed]` resources: the
//!   generated closure extracts every parameter, identity included, before it
//!   snapshots the request head the resolver sees.
//! - **Controller-field tenancy + parameter-level identity supplying the claim**
//!   — **unsupported**, and inherently so: a controller's request-scoped fields
//!   are extracted by a single `FromRequestParts` extractor that necessarily
//!   runs before the handler's own parameters, so the claim is not in the
//!   extensions yet and the resolver sees nothing. It fails the way a missing
//!   tenant fails (the configured `missing-status`, or `None` under
//!   `on-missing = allow`), not silently with a wrong tenant.
//!
//!   Move the identity to the controller struct (and mark the public routes
//!   `#[anonymous]`) when the tenant comes from the identity. A route needing
//!   both shapes can keep the field tenancy and add its own
//!   `#[inject(identity)]` parameter — but the tenant must then come from
//!   somewhere the request already carries (a header, the path, the host).

use std::ops::Deref;

use r2e_core::extract::{FromRequestPartsVia, OptionalFromRequestPartsVia, ViaBean};
use r2e_core::http::Parts;
use r2e_core::type_list::HasBean;
use r2e_core::HttpError;

use crate::map::Tenanted;
use crate::router::TenantRouter;
use crate::TenantId;

/// The current tenant's `T`.
///
/// Derefs to `T`, so a `Tenant<Pool<Postgres>>` is used exactly like the pool it
/// wraps; [`tenant_id`](Self::tenant_id) is there when the handler also needs to
/// know *which* tenant it is serving.
#[derive(Debug, Clone)]
pub struct Tenant<T> {
    tenant: TenantId,
    value: T,
}

impl<T> Tenant<T> {
    /// Pair a resolved tenant with its resource. Useful in tests and in
    /// hand-written extraction.
    pub fn new(tenant: TenantId, value: T) -> Self {
        Self { tenant, value }
    }

    /// The tenant this resource belongs to.
    #[must_use]
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant
    }

    /// The resource.
    #[must_use]
    pub fn get(&self) -> &T {
        &self.value
    }

    /// Take the resource, dropping the tenant id.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.value
    }

    /// Split into the tenant and its resource.
    #[must_use]
    pub fn into_parts(self) -> (TenantId, T) {
        (self.tenant, self.value)
    }
}

impl<T> Deref for Tenant<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> AsRef<T> for Tenant<T> {
    fn as_ref(&self) -> &T {
        &self.value
    }
}

impl<S, T, I, J> FromRequestPartsVia<S, ViaBean<(I, J)>> for Tenant<T>
where
    S: HasBean<TenantRouter, I> + HasBean<Tenanted<T>, J> + Send + Sync,
    T: Clone + Send + Sync + 'static,
    I: Send + Sync,
    J: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts_via(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let router: TenantRouter = state.get_bean();
        let map: Tenanted<T> = state.get_bean();
        let tenant = router.resolve_parts(parts, state).await?;
        let value = map
            .get(&tenant)
            .await
            .map_err(|err| err.into_http_error(map.statuses()))?;
        Ok(Self::new(tenant, value))
    }
}

impl<S, T, I, J> OptionalFromRequestPartsVia<S, ViaBean<(I, J)>> for Tenant<T>
where
    S: HasBean<TenantRouter, I> + HasBean<Tenanted<T>, J> + Send + Sync,
    T: Clone + Send + Sync + 'static,
    I: Send + Sync,
    J: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts_via(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        let router: TenantRouter = state.get_bean();
        let Some(tenant) = router.try_resolve_parts(parts, state).await? else {
            return Ok(None);
        };
        let map: Tenanted<T> = state.get_bean();
        let value = map
            .get(&tenant)
            .await
            .map_err(|err| err.into_http_error(map.statuses()))?;
        Ok(Some(Self::new(tenant, value)))
    }
}

impl<S, I> FromRequestPartsVia<S, ViaBean<I>> for TenantId
where
    S: HasBean<TenantRouter, I> + Send + Sync,
    I: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts_via(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let router: TenantRouter = state.get_bean();
        router.resolve_parts(parts, state).await
    }
}

impl<S, I> OptionalFromRequestPartsVia<S, ViaBean<I>> for TenantId
where
    S: HasBean<TenantRouter, I> + Send + Sync,
    I: Send + Sync,
{
    type Rejection = HttpError;

    async fn from_request_parts_via(
        parts: &mut Parts,
        state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        let router: TenantRouter = state.get_bean();
        router.try_resolve_parts(parts, state).await
    }
}
