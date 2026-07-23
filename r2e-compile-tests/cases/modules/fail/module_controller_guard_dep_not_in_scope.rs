//! A module controller whose guard reads a bean outside the module's scope
//! (neither provided nor imported) must be rejected at `register_module` —
//! decorator deps are part of `EndpointDeps::Deps`, like `#[inject]` deps.

use r2e::prelude::*;
use r2e::type_list::{TCons, TNil};
use r2e::{BeanContext, GuardContext, Identity};
use std::future::Future;

/// The bean the guard needs — not in the module's scope.
#[derive(Clone)]
pub struct Pool;

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

#[controller(path = "/bad")]
pub struct BadController {
    #[inject]
    svc: Svc, // in scope — only the guard dep is not
}

#[routes]
impl BadController {
    #[get("/")]
    #[guard(Audit)]
    async fn index(&self) -> &'static str {
        let _ = &self.svc;
        "bad"
    }
}

pub struct BadModule;

impl FeatureModule for BadModule {
    type Providers = TCons<Svc, TNil>;
    type Controllers = (BadController,);
    type Exports = TCons<Svc, TNil>;
    type Imports = TNil; type RequiredPlugins = ();
}

fn main() {
    let _ = r2e::AppBuilder::new().register_module::<BadModule>();
}
