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

#[r2e_core::test]
async fn build_state_after_raw_load_config_provides_unit_slot() {
    // `load_config::<()>()` pushes `()` onto the provision list; the unit
    // bean must be registered too or materializing the HList panics with
    // "Bean of type `()` not found in context".
    let app = AppBuilder::new().load_config::<()>().build_state().await;
    let _: () = app.state().get::<()>();
    assert!(app
        .bean_context()
        .try_get::<r2e_core::R2eConfig>()
        .is_some());
}

// ── Pre-state builder registration methods ──────────────────────────────────

use r2e_core::beans::{AsyncBean, PostConstruct, PreDestroy, Producer};
use r2e_core::config::{ConfigValue, R2eConfig};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Clone)]
struct Hooked {
    flag: Arc<AtomicBool>,
}

impl PostConstruct for Hooked {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move {
            self.flag.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[r2e_core::test]
async fn provide_with_post_construct_runs_hook_during_build_state() {
    let flag = Arc::new(AtomicBool::new(false));
    let app = AppBuilder::new()
        .provide_with_post_construct(Hooked { flag: flag.clone() })
        .build_state()
        .await;

    assert!(flag.load(Ordering::SeqCst), "post-construct hook must fire");
    let _: Hooked = app.state().get();
}

#[derive(Clone)]
struct Disposed {
    flag: Arc<AtomicBool>,
}

impl PreDestroy for Disposed {
    fn pre_destroy(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.flag.store(true, Ordering::SeqCst);
        })
    }
}

#[r2e_core::test]
async fn provide_with_pre_destroy_defers_hook_to_shutdown() {
    let flag = Arc::new(AtomicBool::new(false));
    let app = AppBuilder::new()
        .provide_with_pre_destroy(Disposed { flag: flag.clone() })
        .build_state()
        .await;

    let _: Disposed = app.state().get();
    // The disposal hook belongs to the shutdown phase — it must not run at
    // build time. (Hook execution order is covered by the registry tests.)
    assert!(!flag.load(Ordering::SeqCst));
}

#[derive(Clone, Debug, PartialEq)]
struct AsyncGreeter {
    salutation: String,
}

impl AsyncBean for AsyncGreeter {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    async fn build(_ctx: &BeanContext) -> Self {
        tokio::task::yield_now().await;
        AsyncGreeter {
            salutation: "async hello".into(),
        }
    }
}

#[r2e_core::test]
async fn with_default_async_bean_builds() {
    let app = AppBuilder::new()
        .with_default_async_bean::<AsyncGreeter>()
        .build_state()
        .await;
    assert_eq!(app.state().get::<AsyncGreeter>().salutation, "async hello");
}

struct GreeterProducer;

impl Producer for GreeterProducer {
    type Output = Greeter;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    async fn produce(_ctx: &BeanContext) -> Greeter {
        Greeter {
            salutation: "produced".into(),
        }
    }
}

#[r2e_core::test]
async fn with_default_producer_builds() {
    let app = AppBuilder::new()
        .with_default_producer::<GreeterProducer>()
        .build_state()
        .await;
    assert_eq!(app.state().get::<Greeter>().salutation, "produced");
}

#[derive(Clone, Debug, PartialEq)]
struct FactoryMade(String);

#[r2e_core::test]
async fn with_bean_factory_reads_config() {
    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::String("factory-app".into()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .with_bean_factory(|config: &R2eConfig| {
            FactoryMade(config.get::<String>("app.name").unwrap())
        })
        .build_state()
        .await;

    assert_eq!(app.state().get::<FactoryMade>().0, "factory-app");
}
