//! `#[bean(lazy)]`: construction deferred to the first `get`.

use std::any::{type_name, TypeId};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use r2e_core::beans::{AsyncBean, Bean, BeanContext, BeanError, BeanRegistry, Producer};
use r2e_core::type_list::TNil;

// ── Lazy beans (`#[bean(lazy)]` runtime path) ───────────────────────────────

use std::sync::atomic::AtomicUsize;

#[derive(Clone)]
pub struct Probe {
    pub calls: Arc<AtomicUsize>,
}

#[derive(Clone)]
pub struct LazyCounter {
    pub n: u32,
}

impl Bean for LazyCounter {
    type Deps = TNil;
    const LAZY: bool = true;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Probe>(), type_name::<Probe>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        let probe = ctx.get::<Probe>();
        probe.calls.fetch_add(1, Ordering::SeqCst);
        LazyCounter { n: 5 }
    }
}

#[r2e_core::test]
async fn lazy_bean_constructed_on_first_get_only() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = BeanRegistry::new();
    reg.provide(Probe {
        calls: calls.clone(),
    });
    reg.register::<LazyCounter>();
    let ctx = reg.resolve().await.unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "lazy bean must not build during resolve"
    );

    // Lazy first-touch uses `block_in_place`, which needs a worker thread.
    let ctx = tokio::spawn(async move {
        assert_eq!(ctx.get::<LazyCounter>().n, 5);
        assert_eq!(ctx.get::<LazyCounter>().n, 5);
        ctx
    })
    .await
    .unwrap();
    drop(ctx);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "factory must run exactly once"
    );
}

#[derive(Clone)]
struct LazyOuter {
    inner_n: u32,
}

impl Bean for LazyOuter {
    type Deps = TNil;
    const LAZY: bool = true;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<LazyCounter>(), type_name::<LazyCounter>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        LazyOuter {
            inner_n: ctx.get::<LazyCounter>().n,
        }
    }
}

#[r2e_core::test]
async fn lazy_to_lazy_dependency_resolves_on_first_get() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = BeanRegistry::new();
    reg.provide(Probe {
        calls: calls.clone(),
    });
    reg.register::<LazyCounter>();
    reg.register::<LazyOuter>();
    let ctx = reg.resolve().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let outer = tokio::spawn(async move { ctx.get::<LazyOuter>() })
        .await
        .unwrap();
    assert_eq!(outer.inner_n, 5);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[derive(Clone)]
struct LazyAsyncThing {
    n: u32,
}

impl AsyncBean for LazyAsyncThing {
    type Deps = TNil;
    const LAZY: bool = true;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    async fn build(_ctx: &BeanContext) -> Self {
        tokio::task::yield_now().await;
        LazyAsyncThing { n: 9 }
    }
}

#[r2e_core::test]
async fn lazy_async_bean_resolves_on_first_get() {
    let mut reg = BeanRegistry::new();
    reg.register_async::<LazyAsyncThing>();
    let ctx = reg.resolve().await.unwrap();

    let thing = tokio::spawn(async move { ctx.get::<LazyAsyncThing>() })
        .await
        .unwrap();
    assert_eq!(thing.n, 9);
}

#[r2e_core::test]
async fn lazy_bean_missing_dependency_errors_at_resolve() {
    let mut reg = BeanRegistry::new();
    // Probe is neither provided nor registered.
    reg.register::<LazyCounter>();
    let err = reg.resolve().await.unwrap_err();
    match &err {
        BeanError::MissingDependency { bean, dependency } => {
            assert!(bean.contains("LazyCounter"), "bean: {bean}");
            assert!(dependency.contains("Probe"), "dependency: {dependency}");
        }
        other => panic!("expected MissingDependency, got {other:?}"),
    }
}

#[r2e_core::test]
async fn lazy_bean_registered_twice_is_duplicate() {
    let mut reg = BeanRegistry::new();
    reg.provide(Probe {
        calls: Arc::new(AtomicUsize::new(0)),
    });
    reg.register::<LazyCounter>();
    reg.register::<LazyCounter>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::DuplicateBean { .. }));
}

#[r2e_core::test]
async fn lazy_bean_conflicting_with_provided_is_duplicate() {
    let mut reg = BeanRegistry::new();
    reg.provide(Probe {
        calls: Arc::new(AtomicUsize::new(0)),
    });
    reg.provide(LazyCounter { n: 1 });
    reg.register::<LazyCounter>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::DuplicateBean { .. }));
}

struct EagerCounterProducer;

impl Producer for EagerCounterProducer {
    type Output = LazyCounter;
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    async fn produce(_ctx: &BeanContext) -> LazyCounter {
        LazyCounter { n: 1 }
    }
}

#[r2e_core::test]
async fn lazy_bean_conflicting_with_eager_registration_is_duplicate() {
    let mut reg = BeanRegistry::new();
    reg.provide(Probe {
        calls: Arc::new(AtomicUsize::new(0)),
    });
    reg.register_producer::<EagerCounterProducer>();
    reg.register::<LazyCounter>();
    let err = reg.resolve().await.unwrap_err();
    assert!(matches!(err, BeanError::DuplicateBean { .. }));
}

#[r2e_core::test]
async fn lazy_default_superseded_by_later_registration() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = BeanRegistry::new();
    reg.provide(Probe {
        calls: calls.clone(),
    });
    reg.register_default::<LazyCounter>();
    // A later registration of the same type silently replaces the default
    // instead of tripping the duplicate check.
    reg.register::<LazyCounter>();
    let ctx = reg.resolve().await.unwrap();

    let n = tokio::spawn(async move { ctx.get::<LazyCounter>().n })
        .await
        .unwrap();
    assert_eq!(n, 5);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[derive(Clone)]
struct LazyCfgRequired;

impl Bean for LazyCfgRequired {
    type Deps = TNil;
    const LAZY: bool = true;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn config_keys() -> Vec<(&'static str, &'static str, bool)> {
        vec![("lazy.greeting", "String", true)]
    }
    fn build(_ctx: &BeanContext) -> Self {
        LazyCfgRequired
    }
}

#[derive(Clone)]
pub struct LazyCfgOptional;

impl Bean for LazyCfgOptional {
    type Deps = TNil;
    const LAZY: bool = true;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn config_keys() -> Vec<(&'static str, &'static str, bool)> {
        vec![("lazy.greeting", "String", false)]
    }
    fn build(_ctx: &BeanContext) -> Self {
        LazyCfgOptional
    }
}

#[r2e_core::test]
async fn lazy_bean_required_config_key_missing_fails_at_resolve() {
    let mut reg = BeanRegistry::new();
    reg.provide(r2e_core::config::R2eConfig::empty());
    reg.register::<LazyCfgRequired>();
    let err = reg.resolve().await.unwrap_err();
    assert!(
        matches!(err, BeanError::MissingConfigKeys(_)),
        "got {err:?}"
    );
}

#[r2e_core::test]
async fn lazy_bean_required_config_key_present_resolves() {
    let mut config = r2e_core::config::R2eConfig::empty();
    config.set(
        "lazy.greeting",
        r2e_core::config::ConfigValue::String("hi".into()),
    );
    let mut reg = BeanRegistry::new();
    reg.provide(config);
    reg.register::<LazyCfgRequired>();
    assert!(reg.resolve().await.is_ok());
}

#[r2e_core::test]
async fn lazy_bean_optional_config_key_absent_is_fine() {
    let mut reg = BeanRegistry::new();
    reg.provide(r2e_core::config::R2eConfig::empty());
    reg.register::<LazyCfgOptional>();
    assert!(reg.resolve().await.is_ok());
}
