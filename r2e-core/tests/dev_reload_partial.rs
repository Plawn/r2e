//! Dev-reload partial rebuild: when the graph fingerprint changes between
//! two `build_state()` cycles in one process (the hot-patch situation),
//! only the changed beans — and their transitive dependents — are
//! reconstructed; every other instance carries over with its in-memory
//! state, and provided values are pinned from the previous cycle.
//!
//! The fingerprint change is driven through a config value (a real
//! production trigger under `r2e dev`), so no actual code patching is
//! needed: the whole flow runs in-process against the dev-reload statics.
//!
//! Everything lives in ONE test function: the dev-reload caches are
//! process-global, so parallel test functions would clobber each other.
#![cfg(feature = "dev-reload")]

use r2e_core::beans::{Bean, BeanContext, BeanRegistry, PostConstruct, Registrable};
use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, TCons, TNil};
use std::any::TypeId;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

static STABLE_BUILDS: AtomicU32 = AtomicU32::new(0);
static STABLE_PCS: AtomicU32 = AtomicU32::new(0);
static CONF_BUILDS: AtomicU32 = AtomicU32::new(0);
static CONF_PCS: AtomicU32 = AtomicU32::new(0);
static USES_STABLE_BUILDS: AtomicU32 = AtomicU32::new(0);
static USES_CONF_BUILDS: AtomicU32 = AtomicU32::new(0);

/// Config-independent bean carrying mutable in-memory state (the counter):
/// the state must survive a partial rebuild triggered by *another* bean.
#[derive(Clone)]
struct Stable {
    counter: Arc<AtomicU32>,
}

impl Bean for Stable {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        STABLE_BUILDS.fetch_add(1, Ordering::SeqCst);
        Self {
            counter: Arc::new(AtomicU32::new(0)),
        }
    }
    fn after_register(registry: &mut BeanRegistry) {
        registry.register_post_construct::<Self>();
    }
}

impl PostConstruct for Stable {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move {
            STABLE_PCS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

impl Registrable for Stable {
    type Provided = Self;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

/// Bean whose fingerprint tracks the `dev.flip` config value: editing the
/// value between cycles is the stand-in for editing its constructor code.
#[derive(Clone)]
struct ConfDep {
    val: String,
}

impl Bean for ConfDep {
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn config_keys() -> Vec<(&'static str, &'static str, bool)> {
        vec![("dev.flip", "String", true)]
    }
    fn build(ctx: &BeanContext) -> Self {
        CONF_BUILDS.fetch_add(1, Ordering::SeqCst);
        let config: R2eConfig = ctx.get();
        Self {
            val: config.get::<String>("dev.flip").unwrap(),
        }
    }
    fn after_register(registry: &mut BeanRegistry) {
        registry.register_post_construct::<Self>();
    }
}

impl PostConstruct for ConfDep {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move {
            CONF_PCS.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

impl Registrable for ConfDep {
    type Provided = Self;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

/// Depends on the config-independent bean → must be reused alongside it.
#[derive(Clone)]
struct UsesStable {
    #[allow(dead_code)]
    inner: Stable,
}

impl Bean for UsesStable {
    type Deps = TCons<Stable, TNil>;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<Stable>(), "Stable")]
    }
    fn build(ctx: &BeanContext) -> Self {
        USES_STABLE_BUILDS.fetch_add(1, Ordering::SeqCst);
        Self { inner: ctx.get() }
    }
}

impl Registrable for UsesStable {
    type Provided = Self;
    type Deps = TCons<Stable, TNil>;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

/// Depends on the config-tracking bean → its fingerprint changes by
/// propagation, so it must rebuild and observe the fresh dependency.
#[derive(Clone)]
struct UsesConf {
    inner: ConfDep,
}

impl Bean for UsesConf {
    type Deps = TCons<ConfDep, TNil>;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<ConfDep>(), "ConfDep")]
    }
    fn build(ctx: &BeanContext) -> Self {
        USES_CONF_BUILDS.fetch_add(1, Ordering::SeqCst);
        Self { inner: ctx.get() }
    }
}

impl Registrable for UsesConf {
    type Provided = Self;
    type Deps = TCons<ConfDep, TNil>;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

/// Provided value (the `.provide()` path — e.g. an `Env`-built pool or a
/// shared store): a partial rebuild must pin the previous cycle's instance
/// so reused and rebuilt beans keep sharing it.
#[derive(Clone, Default)]
struct Store(Arc<Mutex<Vec<String>>>);

impl Store {
    fn contents(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

fn flip_config(value: &str) -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("dev.flip", ConfigValue::String(value.into()));
    config
}

macro_rules! cycle {
    ($flip:expr, $store:expr) => {
        AppBuilder::new()
            .override_config(flip_config($flip))
            .load_config::<()>()
            .provide($store)
            .register::<Stable>()
            .register::<ConfDep>()
            .register::<UsesStable>()
            .register::<UsesConf>()
            .build_state()
            .await
    };
}

#[r2e_core::test]
async fn partial_rebuild_reuses_unchanged_beans_across_cycles() {
    // The caches only engage inside the hot-patch loop (`r2e::launch!` marks
    // it); this test drives the loop's build cycles by hand, so opt in.
    r2e_core::dev::mark_hot_reload_loop();

    // ── Cycle 1: cold start — everything builds once ────────────────────
    let app1 = cycle!("a", Store::default());
    let state1 = app1.state().clone();

    assert_eq!(STABLE_BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(CONF_BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(USES_STABLE_BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(USES_CONF_BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(STABLE_PCS.load(Ordering::SeqCst), 1);
    assert_eq!(CONF_PCS.load(Ordering::SeqCst), 1);
    assert_eq!(state1.get::<ConfDep>().val, "a");

    // Mutate in-memory state that must survive the next (partial) rebuild.
    state1.get::<Stable>().counter.store(7, Ordering::SeqCst);
    state1
        .get::<Store>()
        .0
        .lock()
        .unwrap()
        .push("persisted".into());

    // ── Cycle 2: `dev.flip` edited → only ConfDep's cone rebuilds ───────
    let app2 = cycle!("b", Store::default());
    let state2 = app2.state().clone();

    assert_eq!(STABLE_BUILDS.load(Ordering::SeqCst), 1, "Stable reused");
    assert_eq!(
        USES_STABLE_BUILDS.load(Ordering::SeqCst),
        1,
        "dependent of an unchanged bean reused"
    );
    assert_eq!(CONF_BUILDS.load(Ordering::SeqCst), 2, "ConfDep rebuilt");
    assert_eq!(
        USES_CONF_BUILDS.load(Ordering::SeqCst),
        2,
        "dependent of a changed bean rebuilt by fingerprint propagation"
    );
    assert_eq!(
        STABLE_PCS.load(Ordering::SeqCst),
        1,
        "post_construct must not re-run on a reused instance"
    );
    assert_eq!(
        CONF_PCS.load(Ordering::SeqCst),
        2,
        "post_construct re-runs on a rebuilt instance"
    );

    // Same instance, state intact — not merely "a bean of the same type".
    assert_eq!(state2.get::<Stable>().counter.load(Ordering::SeqCst), 7);
    // The rebuilt cone observed the fresh config.
    assert_eq!(state2.get::<ConfDep>().val, "b");
    assert_eq!(state2.get::<UsesConf>().inner.val, "b");
    // The provided value was pinned from cycle 1; the fresh (empty) Store
    // handed to cycle 2 was discarded.
    assert_eq!(state2.get::<Store>().contents(), vec!["persisted"]);

    // ── Cycle 3: nothing changed → full-reuse fast path, zero factories ─
    let app3 = cycle!("b", Store::default());
    let state3 = app3.state().clone();

    assert_eq!(STABLE_BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(CONF_BUILDS.load(Ordering::SeqCst), 2);
    assert_eq!(USES_CONF_BUILDS.load(Ordering::SeqCst), 2);
    assert_eq!(CONF_PCS.load(Ordering::SeqCst), 2);
    assert_eq!(state3.get::<Stable>().counter.load(Ordering::SeqCst), 7);
    assert_eq!(state3.get::<Store>().contents(), vec!["persisted"]);

    // ── Explicit invalidation: the escape hatch forces a cold rebuild ───
    r2e_core::invalidate_state_cache();
    let app4 = cycle!("b", Store::default());
    let state4 = app4.state().clone();

    assert_eq!(STABLE_BUILDS.load(Ordering::SeqCst), 2);
    assert_eq!(CONF_BUILDS.load(Ordering::SeqCst), 3);
    assert_eq!(STABLE_PCS.load(Ordering::SeqCst), 2);
    assert_eq!(
        state4.get::<Stable>().counter.load(Ordering::SeqCst),
        0,
        "invalidation drops carried state"
    );
    assert!(state4.get::<Store>().contents().is_empty());
}
