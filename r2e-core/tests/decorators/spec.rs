//! Tests for `r2e_core::decorator` — the `DecoratorSpec` construction
//! contract and the `SelfBuilt` blanket.

use r2e_core::beans::{BeanContext, BeanRegistry};
use r2e_core::type_list::{TCons, TNil};
use r2e_core::{DecoratorSpec, SelfBuilt};

trait SameTy<B> {}
impl<A> SameTy<A> for A {}
fn assert_same<A: SameTy<B>, B>() {}

// ── Self-built decorator ────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
struct UnitGuard {
    limit: usize,
}

impl SelfBuilt for UnitGuard {}

#[r2e_core::test]
async fn self_built_blanket_is_identity_with_no_deps() {
    assert_same::<<UnitGuard as DecoratorSpec>::Product, UnitGuard>();
    assert_same::<<UnitGuard as DecoratorSpec>::Deps, TNil>();

    let ctx = BeanRegistry::new().resolve().await.expect("empty graph");
    let built = <UnitGuard as DecoratorSpec>::build(UnitGuard { limit: 3 }, &ctx);
    assert_eq!(built, UnitGuard { limit: 3 });
}

// ── Config-type spec pulling a bean ────────────────────────────────────────

#[derive(Clone)]
struct Backend(&'static str);

struct Cfg {
    name: &'static str,
}

struct Product {
    backend: Backend,
    name: &'static str,
}

impl DecoratorSpec for Cfg {
    type Product = Product;
    type Deps = TCons<Backend, TNil>;

    fn build(self, ctx: &BeanContext) -> Product {
        Product {
            backend: ctx.get(),
            name: self.name,
        }
    }
}

#[r2e_core::test]
async fn config_spec_pulls_beans_from_context() {
    let mut registry = BeanRegistry::new();
    registry.provide(Backend("redis"));
    let ctx = registry.resolve().await.expect("graph must resolve");

    let product = <Cfg as DecoratorSpec>::build(Cfg { name: "site-a" }, &ctx);
    assert_eq!(product.backend.0, "redis");
    assert_eq!(product.name, "site-a");
    assert_same::<<Cfg as DecoratorSpec>::Deps, TCons<Backend, TNil>>();
}
