//! A guard whose `DecoratorSpec::Deps` names a bean the app never provided
//! must be rejected at `register_controller()` — decorator deps are folded
//! into `Controller::Deps` and checked like `#[inject]` deps.

use r2e::prelude::*;
use r2e::type_list::{TCons, TNil};
use r2e::{BeanContext, GuardContext, Identity};
use std::future::Future;

/// The bean the guard needs — deliberately never provided.
#[derive(Clone)]
pub struct QuotaRegistry;

/// Spec named by the attribute.
pub struct Quota {
    max: u64,
}

impl Quota {
    pub fn per_user(max: u64) -> Self {
        Self { max }
    }
}

pub struct QuotaGuard {
    _registry: QuotaRegistry,
    _max: u64,
}

impl DecoratorSpec for Quota {
    type Product = QuotaGuard;
    type Deps = TCons<QuotaRegistry, TNil>;

    fn build(self, ctx: &BeanContext) -> QuotaGuard {
        QuotaGuard {
            _registry: ctx.get(),
            _max: self.max,
        }
    }
}

impl<I: Identity> Guard<I> for QuotaGuard {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

#[controller(path = "/q")]
pub struct QuotaController {}

#[routes]
impl QuotaController {
    #[get("/")]
    #[guard(Quota::per_user(5))]
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
