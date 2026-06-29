//! Controller dispatch micro-benchmark (roadmap Phase 6 "performance proof").
//!
//! Measures controller DISPATCH overhead in isolation from JWT cryptography by
//! comparing equivalent scenarios over an in-process `tower::oneshot` call (no
//! sockets, no live server, no IdP):
//!
//! 1. `axum_bare` — diagnostic bare-Axum reference, without `AppBuilder` layers.
//! 2. `axum_app_stack` — manual Axum handler built through `AppBuilder`, with the
//!    same captured `Arc` core and global layers as the R2E controller.
//! 3. `r2e_no_request_scope` — standard R2E controller with only app-scoped
//!    `#[inject]` fields.
//! 4. `axum_app_stack_param_identity` — manual Axum identity handler through the
//!    same application stack, using the same stub extractor.
//! 5. `r2e_param_identity` — parameter-level `#[inject(identity)]`.
//! 6. `r2e_struct_identity` — struct-level identity in the request façade.
//!
//! The stub identity deliberately performs NO cryptography: it returns a fixed
//! value with no header parsing or signature verification, so the delta between
//! scenarios is pure framework dispatch cost (façade bind, `Arc` clone, extraction
//! plumbing) rather than real JWT/JWKS verification — which is a separate concern.
//!
//! Run a quick pass with:
//! `cargo bench -p r2e-core --bench controller_dispatch -- --warm-up-time 1 --measurement-time 3`

use std::{hint::black_box, sync::Arc};

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
    label: &'static str,
}

struct PlainCore {
    label: &'static str,
}

// ── Standard R2E controller without request-scoped fields ───────────────────

#[controller(state = BenchState)]
struct NoIdentityController {
    #[inject]
    label: &'static str,
}

#[routes]
impl NoIdentityController {
    #[get("/dispatch")]
    async fn handle(&self) -> StatusCode {
        black_box(self.label);
        StatusCode::NO_CONTENT
    }
}

// ── R2E parameter-level identity (stub extractor) ───────────────────────────

#[controller(state = BenchState)]
struct ParamIdentityController {
    #[inject]
    label: &'static str,
}

#[routes]
impl ParamIdentityController {
    #[get("/dispatch")]
    async fn handle(&self, #[inject(identity)] user: StubIdentity) -> StatusCode {
        black_box(self.label);
        black_box(user.0);
        StatusCode::NO_CONTENT
    }
}

// ── R2E struct-level identity façade (stub extractor) ───────────────────────

#[controller(state = BenchState)]
struct StructIdentityController {
    #[inject]
    label: &'static str,
    #[inject(identity)]
    user: StubIdentity,
}

#[routes]
impl StructIdentityController {
    #[get("/dispatch")]
    async fn handle(&self) -> StatusCode {
        black_box(self.label);
        black_box(self.user.0);
        StatusCode::NO_CONTENT
    }
}

// ── Router construction (outside the timed loop) ─────────────────────────────

fn state() -> BenchState {
    BenchState { label: "ok" }
}

fn plain_axum_routes() -> Router<BenchState> {
    let core = Arc::new(PlainCore { label: "ok" });
    Router::new().route(
        "/dispatch",
        get({
            let core = core.clone();
            move || async move {
                black_box(core.label);
                StatusCode::NO_CONTENT
            }
        }),
    )
}

fn plain_axum_identity_routes() -> Router<BenchState> {
    let core = Arc::new(PlainCore { label: "ok" });
    Router::new().route(
        "/dispatch",
        get({
            let core = core.clone();
            move |user: StubIdentity| async move {
                black_box(core.label);
                black_box(user.0);
                StatusCode::NO_CONTENT
            }
        }),
    )
}

fn bare_axum_router() -> Router {
    plain_axum_routes().with_state(state())
}

fn axum_app_stack_router() -> Router {
    r2e_core::AppBuilder::new()
        .with_state(state())
        .register_routes(plain_axum_routes())
        .build()
}

fn axum_app_stack_identity_router() -> Router {
    r2e_core::AppBuilder::new()
        .with_state(state())
        .register_routes(plain_axum_identity_routes())
        .build()
}

fn no_request_scope_router() -> Router {
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
    debug_assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    black_box(resp);
}

fn bench_dispatch(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // Build every router once, outside the timed loop.
    let bare_axum = bare_axum_router();
    let axum_app_stack = axum_app_stack_router();
    let no_request_scope = no_request_scope_router();
    let axum_app_stack_identity = axum_app_stack_identity_router();
    let param_identity = param_identity_router();
    let struct_identity = struct_identity_router();

    let mut group = c.benchmark_group("controller_dispatch");

    group.bench_function("axum_bare", |b| b.iter(|| dispatch(&rt, &bare_axum)));
    group.bench_function("axum_app_stack", |b| {
        b.iter(|| dispatch(&rt, &axum_app_stack))
    });
    group.bench_function("r2e_no_request_scope", |b| {
        b.iter(|| dispatch(&rt, &no_request_scope))
    });
    group.bench_function("axum_app_stack_param_identity", |b| {
        b.iter(|| dispatch(&rt, &axum_app_stack_identity))
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
