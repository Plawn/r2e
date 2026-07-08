//! A module controller whose guard reads a bean the module itself provides
//! must pass the module-scope check — decorator deps resolve against
//! `Provides ∪ Imports` exactly like `#[inject]` deps.

use r2e::prelude::*;
use r2e::type_list::{TCons, TNil};
use r2e::{BeanContext, GuardContext, Identity};
use std::future::Future;

/// The bean the guard needs — provided by the module.
#[derive(Clone)]
pub struct Pool;

#[bean]
impl Pool {
    fn new() -> Self {
        Self
    }
}

#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    fn new() -> Self {
        Self
    }
}

/// Spec named by the attribute.
pub struct Audit;

pub struct AuditGuard {
    _pool: Pool,
}

impl DecoratorSpec for Audit {
    type Product = AuditGuard;
    type Deps = TCons<Pool, TNil>;

    fn build(self, ctx: &BeanContext) -> AuditGuard {
        AuditGuard { _pool: ctx.get() }
    }
}

impl<I: Identity> Guard<I> for AuditGuard {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

#[controller(path = "/good")]
pub struct GoodController {
    #[inject]
    svc: Svc,
}

#[routes]
impl GoodController {
    #[get("/")]
    #[guard(Audit)]
    async fn index(&self) -> &'static str {
        let _ = &self.svc;
        "good"
    }
}

pub struct GoodModule;

impl FeatureModule for GoodModule {
    type Providers = TCons<Pool, TCons<Svc, TNil>>;
    type Controllers = (GoodController,);
    type Exports = TCons<Svc, TNil>;
    type Imports = TNil;
}

fn main() {
    let _ = r2e::AppBuilder::new().register_module::<GoodModule>();
}
