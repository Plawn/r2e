//! Controller dispatch micro-benchmark (roadmap Phase 6 "performance proof").
//!
//! Measures controller DISPATCH overhead in isolation from JWT cryptography by
//! comparing four scenarios over an in-process `tower::oneshot` call (no sockets,
//! no live server, no IdP):
//!
//! 1. `plain_axum`           — baseline Axum handler, no R2E controller.
//! 2. `r2e_no_identity`      — R2E controller with only app-scoped `#[inject]`
//!                             fields (served from the captured `Arc` core, no
//!                             per-request façade).
//! 3. `r2e_param_identity`   — R2E parameter-level `#[inject(identity)]` using a
//!                             STUB extractor (fixed value, no signature check).
//! 4. `r2e_struct_identity`  — R2E struct-level identity façade using the SAME
//!                             stub extractor.
//!
//! The stub identity deliberately performs NO cryptography: it returns a fixed
//! value with no header parsing or signature verification, so the delta between
//! scenarios is pure framework dispatch cost (façade bind, `Arc` clone, extraction
//! plumbing) rather than real JWT/JWKS verification — which is a separate concern.
//!
//! Run a quick pass with:
//! `cargo bench -p r2e-core --bench controller_dispatch -- --warm-up-time 1 --measurement-time 3`

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::response::Response;
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Router, StatusCode};
use r2e_core::prelude::*;
use r2e_core::Identity;
use tower::ServiceExt;

// ── Stub identity: dispatch plumbing only, no cryptography ───────────────────

/// An identity that is extracted with a fixed value and performs NO signature
/// verification, JWKS lookup, or header parsing. This isolates the framework's
/// extraction plumbing from real JWT crypto cost.
struct StubIdentity(&'static str);

impl Identity for StubIdentity {
    fn sub(&self) -> &str {
        self.0
    }
}

impl<S: Send + Sync> FromRequestParts<S> for StubIdentity {
    type Rejection = Response;

    async fn from_request_parts(
        _parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        // Fixed value — no crypto, no header read. Pure plumbing.
        Ok(StubIdentity("bench-subject"))
    }
}

// ── Shared application state ─────────────────────────────────────────────────

#[derive(Clone)]
struct BenchState {
    label: String,
}

// ── Scenario 2: R2E controller WITHOUT identity (app-scoped only) ────────────

#[controller(state = BenchState)]
struct NoIdentityController {
    #[inject]
    label: String,
}

#[routes]
impl NoIdentityController {
    #[get("/dispatch")]
    async fn handle(&self) -> String {
        self.label.clone()
    }
}

// ── Scenario 3: R2E parameter-level identity (stub extractor) ────────────────

#[controller(state = BenchState)]
struct ParamIdentityController {
    #[inject]
    label: String,
}

#[routes]
impl ParamIdentityController {
    #[get("/dispatch")]
    async fn handle(&self, #[inject(identity)] user: StubIdentity) -> String {
        // Touch both the core field and the request-scoped identity.
        format!("{}:{}", self.label, user.0)
    }
}

// ── Scenario 4: R2E struct-level identity façade (stub extractor) ────────────

#[controller(state = BenchState)]
struct StructIdentityController {
    #[inject]
    label: String,
    #[inject(identity)]
    user: StubIdentity,
}

#[routes]
impl StructIdentityController {
    #[get("/dispatch")]
    async fn handle(&self) -> String {
        format!("{}:{}", self.label, self.user.0)
    }
}

// ── Router construction (outside the timed loop) ─────────────────────────────

fn state() -> BenchState {
    BenchState {
        label: "ok".to_string(),
    }
}

fn plain_axum_router() -> Router {
    // Mirrors the R2E handler shape: a GET returning a String, no state.
    Router::new().route("/dispatch", get(|| async { "ok".to_string() }))
}

fn no_identity_router() -> Router {
    r2e_core::AppBuilder::new()
        .with_state(state())
        .register_controller::<NoIdentityController>()
        .build()
}

fn param_identity_router() -> Router {
    r2e_core::AppBuilder::new()
        .with_state(state())
        .register_controller::<ParamIdentityController>()
        .build()
}

fn struct_identity_router() -> Router {
    r2e_core::AppBuilder::new()
        .with_state(state())
        .register_controller::<StructIdentityController>()
        .build()
}

/// Identical request shape across every scenario, so the measured delta is pure
/// framework overhead.
fn request() -> Request<Body> {
    Request::builder()
        .uri("/dispatch")
        .body(Body::empty())
        .unwrap()
}

/// Time a single in-process dispatch: clone the router, drive one request to
/// completion, and assert it succeeded (outside criterion's reported timing the
/// assert is negligible, but keeps the bench honest).
fn dispatch(rt: &tokio::runtime::Runtime, router: &Router) {
    let resp = rt.block_on(router.clone().oneshot(request())).unwrap();
    debug_assert_eq!(resp.status(), StatusCode::OK);
    black_box(resp);
}

fn bench_dispatch(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // Build every router once, outside the timed loop.
    let plain = plain_axum_router();
    let no_identity = no_identity_router();
    let param_identity = param_identity_router();
    let struct_identity = struct_identity_router();

    let mut group = c.benchmark_group("controller_dispatch");

    group.bench_function("plain_axum", |b| {
        b.iter(|| dispatch(&rt, &plain))
    });
    group.bench_function("r2e_no_identity", |b| {
        b.iter(|| dispatch(&rt, &no_identity))
    });
    group.bench_function("r2e_param_identity", |b| {
        b.iter(|| dispatch(&rt, &param_identity))
    });
    group.bench_function("r2e_struct_identity", |b| {
        b.iter(|| dispatch(&rt, &struct_identity))
    });

    group.finish();
}

criterion_group!(benches, bench_dispatch);
criterion_main!(benches);
