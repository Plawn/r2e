//! A hot-patch cycle that fails to assemble must leave nothing behind.
//!
//! `try_build_state()` **stages** its cycle instead of publishing it, and
//! `r2e::launch!` decides: `commit_dev_cycle()` once the whole `App::build`
//! returned `Ok`, `rollback_dev_cycle()` when it returned `Err`. This test
//! plays the loop's part by hand (the loop itself needs the Subsecond runtime,
//! which no test process has) and asserts the three properties the contract
//! rests on:
//!
//! 1. a failed cycle drops the beans it built, right away;
//! 2. the caches keep the LAST SUCCESSFUL cycle — not the failed one — so the
//!    next patch neither reuses a half-assembled graph nor inherits a
//!    lifecycle it did not initialize;
//! 3. `invalidate_state_cache()` also discards a cycle still in staging.

use crate::serial::{dev_serial, CommitCycle};
use r2e_core::beans::{Bean, BeanContext, BeanRegistry, Registrable};
use r2e_core::config::{ConfigKeyKind, ConfigValue, R2eConfig};
use r2e_core::type_list::BeanAccess;
use r2e_core::{AppBuilder, TNil};
use std::any::TypeId;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

static BUILDS: AtomicU32 = AtomicU32::new(0);
static DROPS: AtomicU32 = AtomicU32::new(0);

/// Counts its own release. Held behind an `Arc` inside the bean, so the
/// counter moves when the LAST clone of the bean goes away — which is exactly
/// what "the failed cycle's beans were dropped" means.
struct DropProbe;

impl Drop for DropProbe {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Fingerprinted on `dev.rollback`, so editing that value between cycles
/// stands in for editing the bean's constructor under `r2e dev`.
#[derive(Clone)]
struct Tracked {
    val: String,
    #[allow(dead_code)]
    probe: Arc<DropProbe>,
}

impl Bean for Tracked {
    type Error = ::std::convert::Infallible;
    type Deps = TNil;

    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }

    fn config_keys() -> Vec<(&'static str, &'static str, ConfigKeyKind)> {
        vec![("dev.rollback", "String", ConfigKeyKind::Required)]
    }

    fn build(ctx: &BeanContext) -> ::std::result::Result<Self, Self::Error> {
        BUILDS.fetch_add(1, Ordering::SeqCst);
        let config: R2eConfig = ctx.get();
        ::std::result::Result::Ok(Self {
            val: config.get::<String>("dev.rollback").unwrap(),
            probe: Arc::new(DropProbe),
        })
    }
}

impl Registrable for Tracked {
    type Provided = Self;
    type Deps = TNil;

    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

fn config_with(value: &str) -> R2eConfig {
    let mut config = R2eConfig::empty();
    config.set("dev.rollback", ConfigValue::String(value.into()));
    config
}

macro_rules! build_cycle {
    ($value:expr) => {
        AppBuilder::new()
            .override_config(config_with($value))
            .load_config::<()>()
            .register::<Tracked>()
            .try_build_state()
            .await
            .expect("the graph resolves")
    };
}

#[r2e_core::test]
async fn a_failed_cycle_rolls_back_and_leaves_the_last_good_one_cached() {
    let _serial = dev_serial();
    r2e_core::invalidate_state_cache();
    r2e_core::runtime::dev::mark_hot_reload_loop();
    BUILDS.store(0, Ordering::SeqCst);
    DROPS.store(0, Ordering::SeqCst);

    // ── Cycle 1: a good patch. The loop commits it. ─────────────────────
    let app1 = build_cycle!("one").commit_cycle();
    let state1 = app1.state().clone();
    assert_eq!(BUILDS.load(Ordering::SeqCst), 1);
    assert_eq!(state1.get::<Tracked>().val, "one");
    assert!(
        !r2e_core::has_staged_dev_cycle(),
        "a committed cycle leaves nothing staged"
    );

    // ── Cycle 2: the patch builds a graph, then the rest of `App::build`
    //    fails (a controller whose config does not validate, a plugin that
    //    refuses, a `?` in the app's own assembly).
    {
        let app2 = build_cycle!("two");
        assert_eq!(BUILDS.load(Ordering::SeqCst), 2, "the new cycle did build");
        assert_eq!(app2.state().get::<Tracked>().val, "two");
        assert!(
            r2e_core::has_staged_dev_cycle(),
            "an uncommitted cycle sits in staging"
        );
        assert_eq!(
            DROPS.load(Ordering::SeqCst),
            0,
            "nothing is dropped while the cycle is still alive"
        );
        // `App::build` returned `Err`: the builder (and the state it owns) is
        // dropped on the error path…
    }
    // …and the loop discards what the cycle staged.
    r2e_core::rollback_dev_cycle();

    assert!(
        !r2e_core::has_staged_dev_cycle(),
        "the rollback empties the staging slot"
    );
    assert_eq!(
        DROPS.load(Ordering::SeqCst),
        1,
        "the failed cycle's beans must be released, not retained by the caches"
    );

    // ── Cycle 3: the next patch. The caches still hold cycle 1, so its
    //    fingerprint is a HIT and its instance is reused verbatim — the failed
    //    cycle left no trace to reuse or to skip a lifecycle over.
    let app3 = build_cycle!("one").commit_cycle();
    let state3 = app3.state().clone();

    assert_eq!(
        BUILDS.load(Ordering::SeqCst),
        2,
        "cycle 1 was still cached, so cycle 3 rebuilt nothing"
    );
    assert!(
        Arc::ptr_eq(
            &state1.get::<Tracked>().probe,
            &state3.get::<Tracked>().probe
        ),
        "cycle 3 must reuse cycle 1's instance, not a graph the failed cycle left behind"
    );
    assert_eq!(state3.get::<Tracked>().val, "one");

    // ── The escape hatch also covers a cycle caught mid-flight ──────────
    let app4 = build_cycle!("three");
    assert!(r2e_core::has_staged_dev_cycle());
    drop(app4);
    r2e_core::invalidate_state_cache();
    assert!(
        !r2e_core::has_staged_dev_cycle(),
        "invalidate_state_cache() discards a staged cycle too"
    );
}
