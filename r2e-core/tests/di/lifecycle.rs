//! `PostConstruct` / `PreDestroy` on beans.

use std::any::{type_name, TypeId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use std::sync::Mutex;

use r2e_core::beans::{Bean, BeanContext, BeanError, BeanRegistry, PostConstruct, PreDestroy};
use r2e_core::type_list::TNil;

// ── PostConstruct tests ────────────────────────────────────────────────

#[derive(Clone)]
pub struct InitTracker {
    pub initialized: Arc<AtomicBool>,
}

impl Bean for InitTracker {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        Self {
            initialized: Arc::new(AtomicBool::new(false)),
        }
    }
    fn after_register(registry: &mut BeanRegistry) {
        registry.register_post_construct::<Self>();
    }
}

impl PostConstruct for InitTracker {
    fn post_construct(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move {
            self.initialized.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

#[r2e_core::test]
async fn post_construct_is_called() {
    let mut reg = BeanRegistry::new();
    reg.register::<InitTracker>();
    let ctx = reg.resolve().await.unwrap();

    let tracker: InitTracker = ctx.get();
    assert!(tracker.initialized.load(Ordering::SeqCst));
}

#[derive(Clone)]
struct FailingBean;

impl Bean for FailingBean {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        Self
    }
    fn after_register(registry: &mut BeanRegistry) {
        registry.register_post_construct::<Self>();
    }
}

impl PostConstruct for FailingBean {
    fn post_construct(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
                + Send
                + '_,
        >,
    > {
        Box::pin(async move { Err("init failed".into()) })
    }
}

#[r2e_core::test]
async fn post_construct_error_propagates() {
    let mut reg = BeanRegistry::new();
    reg.register::<FailingBean>();
    let err = reg.resolve().await.unwrap_err();
    match &err {
        BeanError::PostConstruct(msg) => {
            assert!(msg.contains("init failed"), "error: {msg}");
        }
        _ => panic!("expected PostConstruct error, got {:?}", err),
    }
}

// ── Provided-bean lifecycle (Phase 5) ──────────────────────────────────
//
// Post-construct parity for `.provide()`-d / plugin `Provided` beans, plus the
// `PreDestroy` disposal groundwork.

type Log = Arc<Mutex<Vec<&'static str>>>;

/// A factory bean whose post-construct records "factory" in the shared log.
#[derive(Clone)]
struct FactoryInit {
    log: Log,
}

impl Bean for FactoryInit {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Log>(), type_name::<Log>())]
    }
    fn build(ctx: &BeanContext) -> Self {
        Self { log: ctx.get() }
    }
    fn after_register(registry: &mut BeanRegistry) {
        registry.register_post_construct::<Self>();
    }
}

impl PostConstruct for FactoryInit {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move {
            self.log.lock().unwrap().push("factory");
            Ok(())
        })
    }
}

/// A value handed to `.provide()` that opts into a post-construct hook.
#[derive(Clone)]
struct ProvidedInit {
    id: &'static str,
    log: Log,
}

impl PostConstruct for ProvidedInit {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move {
            self.log.lock().unwrap().push(self.id);
            Ok(())
        })
    }
}

#[r2e_core::test]
async fn provided_post_construct_runs_after_factory_post_construct() {
    let log: Log = Arc::new(Mutex::new(Vec::new()));

    let mut reg = BeanRegistry::new();
    reg.provide(log.clone());
    reg.register::<FactoryInit>();
    reg.provide(ProvidedInit {
        id: "provided",
        log: log.clone(),
    });
    reg.register_provided_post_construct::<ProvidedInit>();

    reg.resolve().await.unwrap();

    // Provided hooks run after all factory-bean post-constructs.
    assert_eq!(*log.lock().unwrap(), vec!["factory", "provided"]);
}

#[derive(Clone)]
struct FailingProvided;

impl PostConstruct for FailingProvided {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move { Err("provided init failed".into()) })
    }
}

#[r2e_core::test]
async fn provided_post_construct_error_propagates() {
    let mut reg = BeanRegistry::new();
    reg.provide(FailingProvided);
    reg.register_provided_post_construct::<FailingProvided>();

    let err = reg.resolve().await.unwrap_err();
    match &err {
        BeanError::PostConstruct(msg) => {
            assert!(msg.contains("provided init failed"), "error: {msg}");
        }
        _ => panic!("expected PostConstruct error, got {:?}", err),
    }
}

#[r2e_core::test]
async fn provided_post_construct_runs_against_pinned_override() {
    let log: Log = Arc::new(Mutex::new(Vec::new()));

    let mut reg = BeanRegistry::new();
    // A harness pins its override BEFORE the app provides the real value.
    reg.pin_provide(ProvidedInit {
        id: "override",
        log: log.clone(),
    });
    reg.provide(ProvidedInit {
        id: "app",
        log: log.clone(),
    });
    reg.register_provided_post_construct::<ProvidedInit>();

    reg.resolve().await.unwrap();

    // The hook reads the value from the graph, which holds the override.
    assert_eq!(*log.lock().unwrap(), vec!["override"]);
}

/// A disposable value recording its id on `pre_destroy`.
#[derive(Clone)]
struct Disposable {
    id: &'static str,
    log: Log,
}

impl PreDestroy for Disposable {
    fn pre_destroy(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.log.lock().unwrap().push(self.id);
        })
    }
}

#[derive(Clone)]
struct DisposableB {
    id: &'static str,
    log: Log,
}

impl PreDestroy for DisposableB {
    fn pre_destroy(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.log.lock().unwrap().push(self.id);
        })
    }
}

#[r2e_core::test]
async fn pre_destroy_disposers_run_in_reverse_registration_order() {
    let log: Log = Arc::new(Mutex::new(Vec::new()));

    let mut reg = BeanRegistry::new();
    reg.provide(Disposable {
        id: "first",
        log: log.clone(),
    });
    reg.register_pre_destroy::<Disposable>();
    reg.provide(DisposableB {
        id: "second",
        log: log.clone(),
    });
    reg.register_pre_destroy::<DisposableB>();

    let mut ctx = reg.resolve().await.unwrap();
    let disposers = ctx.take_disposers();
    assert_eq!(disposers.len(), 2);

    // Disposal runs in reverse registration order (last registered first).
    for hook in disposers {
        hook().await;
    }
    assert_eq!(*log.lock().unwrap(), vec!["second", "first"]);
}
