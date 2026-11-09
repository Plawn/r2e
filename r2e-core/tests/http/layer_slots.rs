//! Layer bands (`LayerSlot`): a layer's position in the stack comes from the
//! band it declares, not from where the app wrote `.plugin(..)`.
//!
//! Each probe layer records its name when the request enters it, so the log
//! reads outermost → innermost.

use std::sync::{Arc, Mutex};

use r2e_core::builder::AppBuilder;
use r2e_core::builtins::{Cors, Health, HttpTrace};
use r2e_core::http::middleware::Next;
use r2e_core::http::{Body, Request, Response, Routes, StatusCode};
use r2e_core::plugin::{Plugin, PluginBuildContext, PluginBuildError};
use r2e_core::{LayerSlot, RequestIdPlugin, SecureHeaders};

use crate::support::{raw, send_get};

type Log = Arc<Mutex<Vec<&'static str>>>;

fn probe_layer(log: Log, name: &'static str) -> impl FnOnce(Routes) -> Routes + Send + 'static {
    move |routes| {
        routes.middleware_fn(move |req: Request, next: Next| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(name);
                next.run(req).await
            }
        })
    }
}

/// A plugin contributing one probe layer — in `slot`, or through the
/// band-less `add_layer` when `slot` is `None`. A plugin type installs once
/// per app, hence the `N` discriminant.
struct Probe<const N: usize> {
    name: &'static str,
    slot: Option<LayerSlot>,
    log: Log,
}

impl<const N: usize> Probe<N> {
    fn in_slot(name: &'static str, slot: LayerSlot, log: &Log) -> Self {
        Self {
            name,
            slot: Some(slot),
            log: log.clone(),
        }
    }

    fn plain(name: &'static str, log: &Log) -> Self {
        Self {
            name,
            slot: None,
            log: log.clone(),
        }
    }
}

impl<const N: usize> Plugin for Probe<N> {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        let layer = probe_layer(self.log, self.name);
        match self.slot {
            Some(slot) => ctx.add_layer_in(slot, layer),
            None => ctx.add_layer(layer),
        }
        Ok(())
    }
}

/// A gate that answers `401` to everything, in the `Auth` band.
struct DenyAll;

impl Plugin for DenyAll {
    type Provided = ();
    type Deps = ();
    type Config = ();
    type Controllers = ();

    async fn build(
        self,
        _deps: Self::Deps,
        _config: Option<Self::Config>,
        ctx: &mut PluginBuildContext,
    ) -> Result<Self::Provided, PluginBuildError> {
        ctx.add_layer_in(LayerSlot::Auth, |routes| {
            routes.middleware_fn(|_req: Request, _next: Next| async { StatusCode::UNAUTHORIZED })
        });
        Ok(())
    }
}

fn log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

async fn entered(router: Routes, log: &Log) -> Vec<&'static str> {
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    log.lock().unwrap().clone()
}

fn header<'a>(resp: &'a Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).map(|v| v.to_str().unwrap())
}

const BANDS_OUTER_TO_INNER: [&str; 5] = ["trace", "metrics", "cors", "auth", "app"];

// ── Band order ──────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn bands_order_the_stack_whatever_the_install_order() {
    let forward = log();
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(Probe::<0>::in_slot("trace", LayerSlot::Trace, &forward))
        .plugin(Probe::<1>::in_slot("metrics", LayerSlot::Metrics, &forward))
        .plugin(Probe::<2>::in_slot("cors", LayerSlot::Cors, &forward))
        .plugin(Probe::<3>::in_slot("auth", LayerSlot::Auth, &forward))
        .plugin(Probe::<4>::in_slot("app", LayerSlot::App, &forward))
        .build_state()
        .await
        .build();
    assert_eq!(entered(router, &forward).await, BANDS_OUTER_TO_INNER);

    // The exact opposite install order produces the same stack.
    let backward = log();
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(Probe::<5>::in_slot("app", LayerSlot::App, &backward))
        .plugin(Probe::<6>::in_slot("auth", LayerSlot::Auth, &backward))
        .plugin(Probe::<7>::in_slot("cors", LayerSlot::Cors, &backward))
        .plugin(Probe::<8>::in_slot("metrics", LayerSlot::Metrics, &backward))
        .plugin(Probe::<9>::in_slot("trace", LayerSlot::Trace, &backward))
        .build_state()
        .await
        .build();
    assert_eq!(entered(router, &backward).await, BANDS_OUTER_TO_INNER);
}

#[r2e_core::test]
async fn install_order_still_applies_within_a_band() {
    let log = log();
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(Probe::<0>::in_slot("auth-first", LayerSlot::Auth, &log))
        .plugin(Probe::<1>::in_slot("trace", LayerSlot::Trace, &log))
        .plugin(Probe::<2>::in_slot("auth-second", LayerSlot::Auth, &log))
        .build_state()
        .await
        .build();
    // Later = outer, inside the band only.
    assert_eq!(
        entered(router, &log).await,
        ["trace", "auth-second", "auth-first"]
    );
}

#[r2e_core::test]
async fn add_layer_is_the_app_band() {
    let log = log();
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(Probe::<0>::in_slot("auth", LayerSlot::Auth, &log))
        .plugin(Probe::<1>::plain("plain", &log))
        .plugin(Probe::<2>::in_slot("app", LayerSlot::App, &log))
        .build_state()
        .await
        .build();
    // `plain` shares the `App` band with `app`: install order between them,
    // both inside `auth` although `auth` was installed first.
    assert_eq!(entered(router, &log).await, ["auth", "app", "plain"]);
}

// ── Builder-level layers ────────────────────────────────────────────────────

#[r2e_core::test]
async fn with_layer_is_the_app_band_and_with_layer_in_picks_one() {
    let log = log();
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(Probe::<0>::in_slot("auth", LayerSlot::Auth, &log))
        .plugin(Probe::<1>::in_slot("trace", LayerSlot::Trace, &log))
        .build_state()
        .await
        .with_layer_fn(probe_layer(log.clone(), "with_layer_fn"))
        .with_layer_fn_in(LayerSlot::Cors, probe_layer(log.clone(), "cors"))
        .build();
    // Added last, yet `with_layer_fn` is innermost and the `Cors` one sits
    // between the trace and the gate.
    assert_eq!(
        entered(router, &log).await,
        ["trace", "cors", "auth", "with_layer_fn"]
    );
}

// ── Built-ins declare their band ────────────────────────────────────────────

/// A gate installed *after* CORS and the trace used to end up outside them:
/// its `401` had no CORS headers (the browser hides it from the caller) and no
/// request id.
#[r2e_core::test]
async fn a_gate_rejection_keeps_cors_security_and_request_id_headers() {
    for gate_first in [true, false] {
        let builder = AppBuilder::new().plugin(Health);
        let router = if gate_first {
            builder
                .plugin(DenyAll)
                .plugin(Cors::permissive())
                .plugin(SecureHeaders::default())
                .plugin(HttpTrace::default())
                .build_state()
                .await
                .build()
        } else {
            builder
                .plugin(HttpTrace::default())
                .plugin(SecureHeaders::default())
                .plugin(Cors::permissive())
                .plugin(DenyAll)
                .build_state()
                .await
                .build()
        };
        let resp = raw(
            router,
            "GET",
            "/health",
            &[("origin", "https://app.example")],
            Body::empty(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "gate_first={gate_first}");
        assert_eq!(
            header(&resp, "access-control-allow-origin"),
            Some("*"),
            "gate_first={gate_first}"
        );
        assert!(
            header(&resp, "x-content-type-options").is_some(),
            "gate_first={gate_first}"
        );
        assert!(header(&resp, "x-request-id").is_some(), "gate_first={gate_first}");
    }
}

/// A preflight carries no credentials: CORS answers it before the gate.
#[r2e_core::test]
async fn a_preflight_is_answered_before_the_gate() {
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(Cors::permissive())
        .plugin(DenyAll)
        .build_state()
        .await
        .build();
    let resp = raw(
        router,
        "OPTIONS",
        "/health",
        &[
            ("origin", "https://app.example"),
            ("access-control-request-method", "GET"),
        ],
        Body::empty(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[r2e_core::test]
async fn the_request_id_plugin_is_in_the_trace_band() {
    let router = AppBuilder::new()
        .plugin(Health)
        .plugin(RequestIdPlugin)
        .plugin(DenyAll)
        .build_state()
        .await
        .build();
    let resp = raw(router, "GET", "/health", &[], Body::empty()).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(header(&resp, "x-request-id").is_some());
}
