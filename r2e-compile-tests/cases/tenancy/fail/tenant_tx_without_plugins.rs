//! A `#[managed] TenantTx` reads the `TenantRouter` and the per-tenant pool map
//! out of the state at acquire time, so both are declared in
//! `TxSource::Deps` and folded into the controller's `ManagedDeps`.
//!
//! Forgetting `.plugin(Tenancy::resolver::<_>())` /
//! `.plugin(PerTenant::<Pool<Sqlite>>::from::<_>())` must therefore fail at
//! `register_controller::<_>()` — not with a 503 on the first request of the
//! first tenant, in production, at 3am.

use r2e::prelude::*;
use r2e::r2e_data_sqlx::TenantTx;
use sqlx::Sqlite;

#[controller(path = "/orders")]
pub struct OrderController;

#[routes]
impl OrderController {
    #[post("/")]
    async fn create(&self, #[managed] _tx: &mut TenantTx<'_, Sqlite>) -> &'static str {
        "created"
    }
}

fn main() {
    let _ = async {
        // Neither `Tenancy` nor `PerTenant::<Pool<Sqlite>>` is installed.
        AppBuilder::new()
            .build_state()
            .await
            .register_controller::<OrderController>()
            .build()
    };
}
