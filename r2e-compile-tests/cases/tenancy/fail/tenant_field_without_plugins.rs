//! `Tenant<T>` reads two beans out of the HList state — the `TenantRouter` (to
//! resolve the request's tenant) and the `Tenanted<T>` map (to get that tenant's
//! `T`) — so forgetting `.plugin(Tenancy::resolver::<_>())` /
//! `.plugin(PerTenant::<T>::from::<_>())` must fail at
//! `register_controller::<_>()`, not at the first request from the first tenant
//! in production.
//!
//! This is the tenancy instance of the general "request-scoped extractor needs a
//! bean" diagnostic: the index witnesses live in the extractor's `ViaBean`
//! marker, so the unsatisfied bound is a `HasBean<..>` on the state.

use r2e::prelude::*;
use r2e::tenant::Tenant;

/// Stands in for a per-tenant resource (a pool, a client, …).
#[derive(Clone)]
pub struct Pool;

#[controller(path = "/orders")]
pub struct OrderController {
    #[inject(request)]
    db: Tenant<Pool>,
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn list(&self) -> String {
        format!("{}", self.db.tenant_id())
    }
}

fn main() {
    let _ = async {
        // Neither `Tenancy` nor `PerTenant::<Pool>` is installed.
        AppBuilder::new()
            .build_state()
            .await
            .register_controller::<OrderController>()
            .build()
    };
}
