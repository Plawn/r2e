//! Decorator beans declare their config keys — and the **host** aggregates
//! them.
//!
//! `#[derive(DecoratorBean)]` emits `DecoratorSpec::config_keys()`; a
//! decorator is not a bean registration, so the declaration is folded into the
//! host that owns the site: `Controller::validate_config` for `#[guard]` /
//! `#[intercept]` sites on a `#[routes]` block. A missing key must therefore
//! surface as the controller's aggregated `ConfigValidationError` at
//! `register_controller()` — never as a fail-late panic inside
//! `build_decorator`.

use std::future::Future;

use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::http::response::Response;
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, GuardContext, Identity};

// ── A decorator bean reading config ────────────────────────────────────────

#[derive(ConfigProperties)]
pub struct QuotaSection {
    #[allow(dead_code)]
    window: u64,
}

#[derive(DecoratorBean)]
pub struct ConfiguredGuard {
    #[config("deco.limit")]
    #[allow(dead_code)]
    limit: u64,
    #[config("deco.optional")]
    #[allow(dead_code)]
    optional: Option<u64>,
    #[config_section(prefix = "deco.quota")]
    #[allow(dead_code)]
    quota: QuotaSection,
    #[live_config("deco.live")]
    #[allow(dead_code)]
    live: LiveConfig<u64>,
}

impl<I: Identity> Guard<I> for ConfiguredGuard {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move { Ok(()) }
    }
}

// ── Hosts ──────────────────────────────────────────────────────────────────

#[controller(path = "/guarded")]
pub struct GuardedController {}

#[routes]
impl GuardedController {
    #[get("/")]
    #[guard(ConfiguredGuard::spec())]
    async fn hello(&self) -> String {
        "ok".into()
    }
}

/// Host that ALSO declares a missing key of its own: both must be reported in
/// the same aggregated error.
#[controller(path = "/both")]
pub struct BothController {
    #[config("ctrl.title")]
    #[allow(dead_code)]
    title: String,
}

#[routes]
impl BothController {
    #[get("/")]
    #[guard(ConfiguredGuard::spec())]
    async fn hello(&self) -> String {
        "ok".into()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// The spec declares every kind, but only `Required` is presence-validated:
/// `Optional`/`Section`/`Live` must not appear as missing keys.
#[test]
fn decorator_spec_declares_its_config_keys() {
    let keys = <__R2eSpec_ConfiguredGuard as DecoratorSpec>::config_keys();
    let mut named: Vec<(&str, bool)> = keys
        .iter()
        .map(|(key, _, kind)| (*key, kind.is_required()))
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec![
            ("deco.limit", true),
            ("deco.live", false),
            ("deco.optional", false),
            ("deco.quota", false),
        ]
    );

    // The identity impl on the product carries the same declaration (the
    // `#[guard(Name = <value>)]` escape hatch names that one).
    assert_eq!(
        <ConfiguredGuard as DecoratorSpec>::config_keys().len(),
        keys.len()
    );
}

#[r2e_core::test]
async fn missing_decorator_config_key_fails_controller_validation() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .build_state()
        .await;

    let err = app
        .try_register_controller::<GuardedController>()
        .err()
        .expect("a decorator bean's missing #[config] key must fail registration");
    let msg = err.to_string();

    assert!(
        msg.contains("deco.limit"),
        "the missing key must be named: {msg}"
    );
    assert!(
        msg.contains("ConfiguredGuard"),
        "the report must name the decorator that requires it: {msg}"
    );
    // A `#[config_section]` field is walked by `DecoratorSpec::config_sections()`
    // (the `config_keys()` entry is only the prefix, kind `Section`, and is not
    // presence-validated): the missing key inside the section is reported by
    // key path, in the same aggregated error.
    assert!(
        msg.contains("deco.quota.window"),
        "the decorator's #[config_section] must be walked, not just its prefix: {msg}"
    );
    // Optional / live keys are NOT presence-validated.
    assert!(!msg.contains("deco.optional"), "{msg}");
    assert!(!msg.contains("deco.live"), "{msg}");
}

#[r2e_core::test]
async fn controller_and_decorator_missing_keys_are_reported_together() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .build_state()
        .await;

    let err = app
        .try_register_controller::<BothController>()
        .err()
        .expect("missing keys must fail registration");
    let msg = err.to_string();

    assert!(msg.contains("ctrl.title"), "{msg}");
    assert!(msg.contains("deco.limit"), "{msg}");
}

#[r2e_core::test]
async fn present_decorator_config_key_passes_validation() {
    let mut config = R2eConfig::empty();
    config.set("deco.limit", ConfigValue::Integer(5));
    config.set("deco.quota.window", ConfigValue::Integer(60));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .build_state()
        .await;

    // Registration goes through, building the guard from the graph.
    let app = app
        .try_register_controller::<GuardedController>()
        .expect("all required keys present");
    let _ = app.build();
}
