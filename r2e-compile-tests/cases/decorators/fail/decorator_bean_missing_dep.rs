//! A `#[derive(DecoratorBean)]` guard whose `#[inject]` bean the app never
//! provided must be rejected at `register_controller()` — the derive's
//! `Deps` carrier is folded into `Controller::Deps` like any spec.

use r2e::prelude::*;
use r2e::{GuardContext, Identity};
use std::future::Future;

/// The bean the guard needs — deliberately never provided.
#[derive(Clone)]
pub struct QuotaRegistry;

#[derive(DecoratorBean)]
pub struct QuotaGuard {
    #[inject]
    registry: QuotaRegistry,
    max: u64,
}

impl<I: Identity> Guard<I> for QuotaGuard {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let _ = (&self.registry, self.max);
        async { Ok(()) }
    }
}

#[controller(path = "/q")]
pub struct QuotaController {}

#[routes]
impl QuotaController {
    #[get("/")]
    #[guard(QuotaGuard::spec(5))]
    async fn hello(&self) -> String {
        "ok".into()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_controller::<QuotaController>()
    };
}
