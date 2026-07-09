//! Tests for the HList state path of the builder: `build_state()` /
//! `try_build_state()` materializing the provision list `P` into a value-level
//! HList, the retained `BeanContext`, and `register_override`.

use r2e_core::beans::{Bean, BeanContext, BeanRegistry, Registrable};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, TNil};
use std::any::TypeId;

#[derive(Clone, Debug, PartialEq)]
struct Dep(u32);

#[derive(Clone, Debug, PartialEq)]
struct Greeter {
    salutation: String,
}

impl Bean for Greeter {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        Greeter {
            salutation: "hello".into(),
        }
    }
}

impl Registrable for Greeter {
    type Provided = Self;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

/// Same provided type as [`Greeter`]'s default, used as an override.
#[derive(Clone)]
struct LoudGreeter;

impl Bean for LoudGreeter {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        LoudGreeter
    }
}

#[r2e_core::test]
async fn build_state_materializes_hlist_from_provisions() {
    let app = AppBuilder::new()
        .provide(Dep(42))
        .register::<Greeter>()
        .build_state()
        .await;

    let state = app.state();
    assert_eq!(state.get::<Dep>(), Dep(42));
    assert_eq!(state.get::<Greeter>().salutation, "hello");
}

#[r2e_core::test]
async fn build_state_retains_bean_context() {
    let app = AppBuilder::new().provide(Dep(7)).build_state().await;
    // NB: `.as_ref()` so the inherent `BeanContext::get` wins over the
    // blanket `BeanAccess::get` (which would otherwise bind at the `Arc`).
    let ctx = app.bean_context();
    assert_eq!(ctx.as_ref().get::<Dep>(), Dep(7));
}

#[r2e_core::test]
async fn try_build_state_reports_duplicate_registration() {
    let err = AppBuilder::new()
        .register::<Greeter>()
        .register::<Greeter>()
        .try_build_state()
        .await
        .map(|_| ())
        .unwrap_err();
    assert!(
        err.to_string().contains("Greeter"),
        "unexpected error: {err}"
    );
}

#[r2e_core::test]
async fn register_override_replaces_default_without_growing_the_state() {
    // A default registration puts Greeter in the provision list once; the
    // override replaces the construction recipe without adding a second slot,
    // so `state.get::<Greeter>()` stays unambiguous.
    struct OverrideGreeter;
    impl r2e_core::beans::Producer for OverrideGreeter {
        type Output = Greeter;
        type Deps = TNil;
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![]
        }
        async fn produce(_ctx: &BeanContext) -> Greeter {
            Greeter {
                salutation: "LOUD HELLO".into(),
            }
        }
    }
    impl Registrable for OverrideGreeter {
        type Provided = Greeter;
        type Deps = TNil;
        fn register_into(registry: &mut BeanRegistry) {
            registry.register_producer::<Self>();
        }
    }

    let app = AppBuilder::new()
        .with_default_bean::<Greeter>()
        .register_override::<OverrideGreeter>()
        .build_state()
        .await;

    assert_eq!(app.state().get::<Greeter>().salutation, "LOUD HELLO");
}

#[r2e_core::test]
async fn build_state_empty_builder_yields_hnil_state() {
    let app = AppBuilder::new().build_state().await;
    let _: &r2e_core::HNil = app.state();
}
