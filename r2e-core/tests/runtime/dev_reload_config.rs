//! The **config** semantics of a dev-reload cycle.
//!
//! `dev_reload.rs` covers bean reuse; this module pins down what happens to
//! the *config* surface — `R2eConfig`, `LiveConfigRegistry`, typed
//! `ConfigProperties` beans, late overrides and config providers — when
//! `build_state()` runs a second time inside the hot-patch loop.
//!
//! The contract these tests lock down is the two-modes model:
//!
//! - a **copied** value (`#[config]`) is read once into the bean, so its
//!   freshness comes from a *rebuild* — the key is fingerprinted and an edit
//!   reconstructs the declaring bean and its dependents;
//! - a **subscribed** value (`#[live_config]`) lives in a registry slot, so its
//!   freshness comes from a *push* — the key is NOT fingerprinted and an edit
//!   reaches existing handles without rebuilding anything.
//!
//! That only works because the `LiveConfigRegistry` has **one identity per
//! process**: `load_config` reuses the carried instance across hot-patch cycles
//! and re-seeds it from the fresh boot config.
//!
//! The full reference (Q1–Q7, evidence, instance diagram) lives in
//! `docs/claude/dev-reload-config-semantics.md`.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use r2e_core::config::{
    ConfigError, ConfigProvider, ConfigProviderContext, ConfigUpdateSink, ConfigValue,
    ConfigWatchContext, LiveConfig, LiveConfigRegistry, R2eConfig,
};
use r2e_core::prelude::Bean;
use r2e_core::type_list::BeanAccess;
use r2e_core::AppBuilder;

use crate::dev_serial::dev_serial;

fn config_with(live: &str, unrelated: &str) -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("app.live", ConfigValue::String(live.into()));
    config.set("app.unrelated", ConfigValue::String(unrelated.into()));
    config
}

/// Q1/Q5 — instance identity of the `LiveConfigRegistry` across cycles.
///
/// The registry is carried in `dev.rs` alongside the other hot-patch statics,
/// so every cycle's `load_config` reuses the same instance and merely re-seeds
/// its boot snapshot. Identity is stable (a runtime `set` from cycle 1 is still
/// readable), and an edited boot value is pushed into the already-materialized
/// slots — so `#[live_config]` readers track a YAML edit just like `#[config]`
/// readers do.
#[r2e_core::test]
async fn live_config_registry_keeps_one_identity_and_reseeds_across_cycles() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::dev::mark_hot_reload_loop();

    // ── Cycle 1 ─────────────────────────────────────────────────────────
    let app1 = AppBuilder::new()
        .override_config(config_with("one", "x"))
        .load_config::<()>()
        .build_state()
        .await;
    let state1 = app1.state().clone();
    let registry1 = state1.get::<LiveConfigRegistry>();
    assert_eq!(registry1.get::<String>("app.live").unwrap(), "one");

    // A key that exists in NO config: only this registry instance can ever
    // answer it, so it doubles as an identity witness below.
    assert!(registry1.set("marker.cycle1", "yes"));

    // ── Cycle 2: the "YAML was edited" hot patch ────────────────────────
    let app2 = AppBuilder::new()
        .override_config(config_with("two", "y"))
        .load_config::<()>()
        .build_state()
        .await;
    let state2 = app2.state().clone();

    // The immutable snapshot bean is refreshed: `R2eConfig` is config-derived
    // and therefore never pinned from the previous cycle.
    assert_eq!(
        state2.get::<R2eConfig>().get::<String>("app.live").unwrap(),
        "two"
    );

    let registry2 = state2.get::<LiveConfigRegistry>();
    // Identity: one registry per process — the runtime write from cycle 1 is
    // still there, so handles bound in cycle 1 keep pointing at a live slot.
    assert_eq!(registry2.get::<String>("marker.cycle1").unwrap(), "yes");

    // Freshness: the re-seed pushed the edited boot value into the slot that
    // cycle 1 materialized.
    assert_eq!(
        registry2.get::<String>("app.live").unwrap(),
        "two",
        "an edited boot value must be pushed into materialized slots"
    );
}

/// Outside the hot-patch loop (production, and any test that never marks it)
/// nothing is carried: each `load_config` builds its own registry, exactly as
/// before the carrier existed.
#[r2e_core::test]
async fn without_the_hot_reload_gate_each_load_config_builds_its_own_registry() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    // Deliberately the *production* path: no `mark_hot_reload_loop()`.
    r2e_core::dev::unmark_hot_reload_loop();

    let app1 = AppBuilder::new()
        .override_config(config_with("one", "x"))
        .load_config::<()>()
        .build_state()
        .await;
    let registry1 = app1.state().get::<LiveConfigRegistry>();
    assert!(registry1.set("marker.first", "yes"));

    let app2 = AppBuilder::new()
        .override_config(config_with("two", "y"))
        .load_config::<()>()
        .build_state()
        .await;
    let registry2 = app2.state().get::<LiveConfigRegistry>();

    assert_eq!(registry2.get::<String>("app.live").unwrap(), "two");
    assert!(
        registry2.get::<String>("marker.first").is_err(),
        "a fresh registry must not see the previous one's runtime writes"
    );
    // …and the first one is untouched by the second boot.
    assert_eq!(registry1.get::<String>("app.live").unwrap(), "one");
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct DevSettings {
    greeting: String,
}

/// Q2 — the staleness hole that is NOT about live config: typed config.
///
/// `LoadableConfig for T: ConfigProperties` registers the typed struct with
/// `registry.provide(typed)` (`config/mod.rs`), which makes it a *provided
/// value* and would normally pin it from the previous cycle exactly like a
/// `.provide()`d pool. It is registered inside a `config_derived_scope`
/// instead, so it is rebuilt from the fresh config every cycle.
#[r2e_core::test]
async fn typed_config_bean_tracks_edits_across_cycles() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::dev::mark_hot_reload_loop();

    let mut config1 = R2eConfig::empty();
    config1.set("greeting", ConfigValue::String("hello".into()));
    let app1 = AppBuilder::new()
        .override_config(config1)
        .load_config::<DevSettings>()
        .build_state()
        .await;
    assert_eq!(app1.state().get::<DevSettings>().greeting, "hello");

    let mut config2 = R2eConfig::empty();
    config2.set("greeting", ConfigValue::String("bonjour".into()));
    let app2 = AppBuilder::new()
        .override_config(config2)
        .load_config::<DevSettings>()
        .build_state()
        .await;
    let state2 = app2.state().clone();

    // The raw config bean tracks the edit…
    assert_eq!(
        state2.get::<R2eConfig>().get::<String>("greeting").unwrap(),
        "bonjour"
    );
    // …and so does the typed section bean built from it.
    assert_eq!(
        state2.get::<DevSettings>().greeting,
        "bonjour",
        "a config-derived provided value must be rebuilt, not pinned"
    );
}

/// Q4/Q5 — `override_config_value` called *after* `load_config` patches
/// `shared.live_config`, which is now the same instance the beans hold on every
/// cycle. So a late override reaches live readers, and its pin survives the
/// re-seed (a YAML value must not silently win over an explicit override).
#[r2e_core::test]
async fn late_override_config_value_reaches_live_readers_and_stays_pinned() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::dev::mark_hot_reload_loop();

    let mut config1 = R2eConfig::empty();
    config1.set("db.url", ConfigValue::String("postgres://boot1".into()));
    let app1 = AppBuilder::new()
        .override_config(config1)
        .load_config::<()>()
        .build_state()
        .await;
    assert_eq!(
        app1.state()
            .get::<LiveConfigRegistry>()
            .get::<String>("db.url")
            .unwrap(),
        "postgres://boot1"
    );

    let mut config2 = R2eConfig::empty();
    config2.set("db.url", ConfigValue::String("postgres://boot2".into()));
    let app2 = AppBuilder::new()
        .override_config(config2)
        .load_config::<()>()
        .override_config_value("db.url", "postgres://late")
        .build_state()
        .await;
    let state2 = app2.state().clone();

    assert_eq!(
        state2.get::<R2eConfig>().get::<String>("db.url").unwrap(),
        "postgres://late"
    );
    let registry = state2.get::<LiveConfigRegistry>();
    assert_eq!(
        registry.get::<String>("db.url").unwrap(),
        "postgres://late",
        "the late override must reach the registry the beans hold"
    );
    // Pinned: a runtime provider push cannot clobber an explicit override.
    assert!(
        !registry.set("db.url", "postgres://from-provider"),
        "an overridden key stays pinned against runtime writes"
    );
    assert_eq!(
        registry.get::<String>("db.url").unwrap(),
        "postgres://late"
    );
}

/// A bean holding only a subscribed value. Its `identity` field is a fresh
/// `Arc` per construction, so `Arc::ptr_eq` distinguishes "same instance
/// carried over" from "rebuilt with the same contents".
#[derive(Clone, Bean)]
struct LiveOnly {
    #[live_config("app.live")]
    url: LiveConfig<String>,
    #[default]
    identity: Arc<AtomicU32>,
}

/// The copied counterpart: same shape, but the value is read once at build.
#[derive(Clone, Bean)]
struct CopiedOnly {
    #[config("app.copied")]
    value: String,
    #[default]
    identity: Arc<AtomicU32>,
}

/// The Phase-2 payoff: a `#[live_config]` key is **not** in the declaring
/// bean's fingerprint, so editing it reaches the handle through the registry
/// push without reconstructing anything — while a `#[config]` edit still
/// rebuilds exactly the beans that copy it.
#[r2e_core::test]
async fn live_key_edit_pushes_without_rebuilding_copied_key_edit_rebuilds() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::dev::mark_hot_reload_loop();

    macro_rules! cycle {
        ($live:expr, $copied:expr) => {{
            let mut config = R2eConfig::empty();
            config.set("app.live", ConfigValue::String($live.into()));
            config.set("app.copied", ConfigValue::String($copied.into()));
            AppBuilder::new()
                .override_config(config)
                .load_config::<()>()
                .register::<LiveOnly>()
                .register::<CopiedOnly>()
                .build_state()
                .await
                .state()
                .clone()
        }};
    }

    // ── Cycle 1 ─────────────────────────────────────────────────────────
    let state1 = cycle!("one", "c1");
    let live1 = state1.get::<LiveOnly>();
    let copied1 = state1.get::<CopiedOnly>();
    assert_eq!(live1.url.get().unwrap(), "one");
    assert_eq!(copied1.value, "c1");

    // ── Cycle 2: only the LIVE key changed ──────────────────────────────
    let state2 = cycle!("two", "c1");
    let live2 = state2.get::<LiveOnly>();
    let copied2 = state2.get::<CopiedOnly>();

    assert!(
        Arc::ptr_eq(&live1.identity, &live2.identity),
        "a live-key edit must not rebuild the bean that subscribes to it"
    );
    assert!(
        Arc::ptr_eq(&copied1.identity, &copied2.identity),
        "an unrelated bean must not be rebuilt either"
    );
    assert_eq!(
        live2.url.get().unwrap(),
        "two",
        "the carried handle must see the edit through the registry push"
    );

    // ── Cycle 3: only the COPIED key changed ────────────────────────────
    let state3 = cycle!("two", "c2");
    let live3 = state3.get::<LiveOnly>();
    let copied3 = state3.get::<CopiedOnly>();

    assert!(
        !Arc::ptr_eq(&copied2.identity, &copied3.identity),
        "a copied-key edit must rebuild the bean that declares it"
    );
    assert_eq!(copied3.value, "c2");
    assert!(
        Arc::ptr_eq(&live2.identity, &live3.identity),
        "a bean that does not declare the edited key must be reused"
    );
    assert_eq!(live3.url.get().unwrap(), "two");
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct DbSection {
    url: String,
    #[config(default = 5)]
    pool_size: i64,
}

/// A bean that copies a whole typed **section** out of the config.
#[derive(Clone, Bean)]
struct SectionHolder {
    #[config_section(prefix = "db")]
    db: DbSection,
    #[default]
    identity: Arc<AtomicU32>,
}

/// `#[config_section]` covers a dotted **prefix**, not one exact key, so the
/// declaring bean's `config_keys()` entry is `ConfigKeyKind::Section` and the
/// fingerprint hashes every key under that prefix. Editing any field inside the
/// section must therefore rebuild the holder; editing anything outside it must
/// not.
///
/// Before this, `#[config_section]` emitted no `config_keys()` entry at all: the
/// bean's fingerprint never moved and `r2e dev` handed it back with the stale
/// struct inside.
#[r2e_core::test]
async fn section_key_edit_rebuilds_the_declaring_bean() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::dev::mark_hot_reload_loop();

    macro_rules! cycle {
        ($pool:expr, $unrelated:expr) => {{
            let mut config = R2eConfig::empty();
            config.set("db.url", ConfigValue::String("postgres://boot".into()));
            config.set("db.pool_size", ConfigValue::Integer($pool));
            config.set("app.unrelated", ConfigValue::String($unrelated.into()));
            AppBuilder::new()
                .override_config(config)
                .load_config::<()>()
                .register::<SectionHolder>()
                .build_state()
                .await
                .state()
                .clone()
        }};
    }

    // ── Cycle 1 ─────────────────────────────────────────────────────────
    let state1 = cycle!(10, "x");
    let holder1 = state1.get::<SectionHolder>();
    assert_eq!(holder1.db.url, "postgres://boot");
    assert_eq!(holder1.db.pool_size, 10);

    // ── Cycle 2: a key INSIDE the section changed ───────────────────────
    let state2 = cycle!(20, "x");
    let holder2 = state2.get::<SectionHolder>();
    assert!(
        !Arc::ptr_eq(&holder1.identity, &holder2.identity),
        "a key edited inside the section must rebuild the bean that copies it"
    );
    assert_eq!(
        holder2.db.pool_size, 20,
        "the rebuilt bean must hold the fresh section"
    );

    // ── Cycle 3: only a key OUTSIDE the section changed ─────────────────
    let state3 = cycle!(20, "y");
    let holder3 = state3.get::<SectionHolder>();
    assert!(
        Arc::ptr_eq(&holder2.identity, &holder3.identity),
        "an edit outside the section prefix must leave the bean reused"
    );
    assert_eq!(holder3.db.pool_size, 20);
}

/// A runtime push (what a `ConfigProvider`'s watch task does) must survive an
/// unrelated reload cycle: the re-seed only pushes boot values that actually
/// *changed*, so an unchanged YAML value never reverts a live write. An edit to
/// that key does win, though — the explicit source of truth moved.
#[r2e_core::test]
async fn runtime_push_survives_an_unrelated_cycle_but_a_real_edit_wins() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::dev::mark_hot_reload_loop();

    let app1 = AppBuilder::new()
        .override_config(config_with("one", "x"))
        .load_config::<()>()
        .build_state()
        .await;
    let registry1 = app1.state().get::<LiveConfigRegistry>();
    assert_eq!(registry1.get::<String>("app.live").unwrap(), "one");
    assert!(registry1.set("app.live", "pushed-at-runtime"));

    // ── Cycle 2: an unrelated key changed ───────────────────────────────
    let state2 = AppBuilder::new()
        .override_config(config_with("one", "y"))
        .load_config::<()>()
        .build_state()
        .await
        .state()
        .clone();
    assert_eq!(
        state2
            .get::<LiveConfigRegistry>()
            .get::<String>("app.live")
            .unwrap(),
        "pushed-at-runtime",
        "an unchanged boot value must not revert a runtime push"
    );

    // ── Cycle 3: the key itself was edited in YAML ──────────────────────
    let state3 = AppBuilder::new()
        .override_config(config_with("three", "y"))
        .load_config::<()>()
        .build_state()
        .await
        .state()
        .clone();
    assert_eq!(
        state3
            .get::<LiveConfigRegistry>()
            .get::<String>("app.live")
            .unwrap(),
        "three",
        "an edited boot value wins over the previous runtime push"
    );
}

static PROVIDER_LOADS: AtomicU32 = AtomicU32::new(0);
static PROVIDER_WATCHES: AtomicU32 = AtomicU32::new(0);

#[derive(Clone)]
struct CountingProvider;

impl ConfigProvider for CountingProvider {
    fn load(
        &self,
        config: &mut R2eConfig,
        _ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError> {
        PROVIDER_LOADS.fetch_add(1, Ordering::SeqCst);
        config.set("provider.key", ConfigValue::String("from-load".into()));
        Ok(())
    }

    fn watch(
        self: Arc<Self>,
        _ctx: ConfigWatchContext,
        sink: ConfigUpdateSink,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConfigError>> + Send + 'static>> {
        Box::pin(async move {
            PROVIDER_WATCHES.fetch_add(1, Ordering::SeqCst);
            sink.set("provider.key", "from-watch");
            Ok(())
        })
    }
}

/// Q4 — watch-task lifecycle across a hot patch.
///
/// `App::build` (hence `load_config`) and every deferred action re-run per
/// cycle, so a *new* watch hook is registered each time; but
/// `PreparedApp::run_inner` skips serve hooks once
/// `dev::is_lifecycle_initialized()`, so only the cycle-1 watch task ever runs
/// (B4 — still characterization, not desired). It writes into the carried
/// registry, which is the one every later cycle keeps using, so its updates
/// stay visible.
#[r2e_core::test]
async fn characterize_provider_watch_runs_once_but_its_registry_is_the_live_one() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::dev::mark_hot_reload_loop();
    PROVIDER_LOADS.store(0, Ordering::SeqCst);
    PROVIDER_WATCHES.store(0, Ordering::SeqCst);

    macro_rules! serve_cycle {
        ($unrelated:expr) => {{
            let mut config = R2eConfig::empty();
            config.set("app.unrelated", ConfigValue::String($unrelated.into()));
            let app = AppBuilder::new()
                .override_config(config)
                .with_config_provider(CountingProvider)
                .load_config::<()>()
                .build_state()
                .await;
            let state = app.state().clone();
            let prepared = app.prepare("127.0.0.1:0");
            let stop = prepared.stop_handle();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let server = tokio::spawn(async move {
                let _ = prepared.run_with_listener(listener).await;
            });
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            stop.stop();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), server).await;
            state
        }};
    }

    // ── Cycle 1: load + watch both run ──────────────────────────────────
    let state1 = serve_cycle!("x");
    assert_eq!(PROVIDER_LOADS.load(Ordering::SeqCst), 1);
    assert_eq!(PROVIDER_WATCHES.load(Ordering::SeqCst), 1);
    assert_eq!(
        state1
            .get::<LiveConfigRegistry>()
            .get::<String>("provider.key")
            .unwrap(),
        "from-watch"
    );

    // ── Cycle 2: the hot patch re-serves ────────────────────────────────
    let state2 = serve_cycle!("y");

    // `load` is part of `load_config`, so it re-runs every cycle…
    assert_eq!(PROVIDER_LOADS.load(Ordering::SeqCst), 2);
    // …while `watch` is an `on_serve` hook, and serve hooks are skipped once
    // the lifecycle is initialized. CURRENT: exactly one watch task exists
    // for the whole dev session (B4).
    assert_eq!(
        PROVIDER_WATCHES.load(Ordering::SeqCst),
        1,
        "serve hooks do not re-run on later hot-patch cycles"
    );

    // The freshly loaded config carries the provider's *boot* value…
    assert_eq!(
        state2
            .get::<R2eConfig>()
            .get::<String>("provider.key")
            .unwrap(),
        "from-load"
    );
    // …while the carried registry still carries the watch update: the boot
    // value is unchanged between the two cycles, so the re-seed leaves the
    // runtime push alone.
    assert_eq!(
        state2
            .get::<LiveConfigRegistry>()
            .get::<String>("provider.key")
            .unwrap(),
        "from-watch"
    );

    // And the surviving cycle-1 sink still reaches the state's registry.
    let sink = ConfigUpdateSink::new(state2.get::<LiveConfigRegistry>());
    assert!(sink.set("provider.key", "from-watch-later"));
    assert_eq!(
        state2
            .get::<LiveConfigRegistry>()
            .get::<String>("provider.key")
            .unwrap(),
        "from-watch-later"
    );
}
