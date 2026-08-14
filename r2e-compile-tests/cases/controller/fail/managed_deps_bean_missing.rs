//! A `#[managed]` resource that looks a bean up at acquire time declares it in
//! `ManagedDeps::Deps`; `#[routes]` folds that into the controller's dependency
//! list, so a bean that was never provided fails at `register_controller()`
//! instead of surfacing as a 500 on the first request.

use r2e::prelude::*;
use r2e::{
    HttpError, ManagedContext, ManagedDeps, ManagedErr, ManagedOutcome, ManagedResource, TCons,
    TNil,
};

/// Stands in for a connection pool bean (a real `Pool<Sqlite>` would drag the
/// backend feature into this case for no extra coverage).
#[derive(Clone)]
pub struct Pool;

pub struct Tx;

impl<S: BeanLookup + Send + Sync> ManagedResource<S> for Tx {
    type Error = ManagedErr<HttpError>;

    async fn acquire(context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        let _pool: Pool = context
            .state
            .bean::<Pool>()
            .ok_or_else(|| context.missing_bean("pool", "Pool", "provide it"))?;
        Ok(Self)
    }

    async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        Ok(())
    }

    fn abort(&mut self) {}
}

impl ManagedDeps for Tx {
    type Deps = TCons<Pool, TNil>;
}

#[controller(path = "/tx")]
pub struct TxController;

#[routes]
impl TxController {
    #[get("/")]
    async fn run(&self, #[managed] _tx: &mut Tx) -> String {
        "ok".to_string()
    }
}

#[tokio::main]
async fn main() {
    // `Pool` is never provided.
    let _router = AppBuilder::new()
        .build_state()
        .await
        .register_controller::<TxController>()
        .build();
}
