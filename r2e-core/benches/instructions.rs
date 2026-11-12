//! Instruction counts for the paths every application pays for: dispatching a
//! request to a controller, and building the bean graph.
//!
//! This is the CPU half of the performance gate; `tests/perf_budget` is the
//! allocation half. Each benchmark runs under Valgrind's Callgrind, which
//! counts executed instructions instead of measuring time, so a result does
//! not depend on how busy the machine is and two runs of the same binary agree
//! to a fraction of a percent. `scripts/perf-instructions.sh` runs this file
//! on a pull request's base and on its head, and fails when a count grows past
//! the threshold set there.
//!
//! Only the body of a `#[library_benchmark]` function is counted: the argument
//! expression in `#[bench::…(…)]` — runtime, router, requests, warm-up — is
//! evaluated before counting starts.
//!
//! The file builds everywhere (`cargo check --all-targets` covers it), but
//! running it needs Linux, Valgrind and a `gungraun-runner` matching the
//! `gungraun` version in `Cargo.lock`:
//!
//! ```bash
//! cargo install gungraun-runner --version <the locked gungraun version>
//! cargo bench -p r2e-core --bench instructions
//! ```
//!
//! For wall-clock numbers, use the criterion bench next to this one
//! (`controller_dispatch`).

#![recursion_limit = "256"]

use std::{hint::black_box, sync::Arc};

// Named imports: `main` would otherwise clash with the `#[main]` attribute
// of the r2e prelude.
use gungraun::{library_benchmark, library_benchmark_group, main};
use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::response::Response;
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, Identity};
use tokio::runtime::Runtime;
use tower::ServiceExt;

const PATH: &str = "/dispatch";

/// Requests dispatched by one `dispatch` benchmark. Enough for the
/// per-request cost to dominate the fixed cost of entering the runtime.
const REQUESTS: usize = 100;

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

// ── Request-scoped fixtures: plumbing only, no parsing, no cryptography ────

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
        Ok(StubIdentity("bench-subject"))
    }
}

struct Locale(&'static str);

impl<S: Send + Sync> FromRequestParts<S> for Locale {
    type Rejection = Response;

    async fn from_request_parts(
        _parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Locale("en"))
    }
}

struct Attempt(u8);

impl<S: Send + Sync> FromRequestParts<S> for Attempt {
    type Rejection = Response;

    async fn from_request_parts(
        _parts: &mut r2e_core::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Attempt(1))
    }
}

// ── Dispatch scenarios ─────────────────────────────────────────────────────

/// The reference: a hand-written handler capturing an `Arc` core, on the same
/// `AppBuilder` stack as the controllers. A controller is expected to cost
/// about the same.
#[derive(Clone)]
struct PlainState;

struct PlainCore {
    label: &'static str,
}

fn plain_handler_router() -> Routes {
    let core = Arc::new(PlainCore { label: "ok" });
    let routes: Routes<PlainState> = Routes::new().route(
        PATH,
        get(move |_: PlainState, _| async move {
            black_box(core.label);
            StatusCode::NO_CONTENT
        }),
    );
    AppBuilder::new()
        .with_state(PlainState)
        .register_routes(routes)
        .build()
}

#[controller]
struct AppScoped {
    #[inject]
    label: &'static str,
}

#[routes]
impl AppScoped {
    #[get("/dispatch")]
    async fn handle(&self) -> StatusCode {
        black_box(self.label);
        StatusCode::NO_CONTENT
    }
}

#[controller]
struct StructIdentity {
    #[inject]
    label: &'static str,
    #[inject(identity)]
    user: StubIdentity,
}

#[routes]
impl StructIdentity {
    #[get("/dispatch")]
    async fn handle(&self) -> StatusCode {
        black_box(self.label);
        black_box(self.user.0);
        StatusCode::NO_CONTENT
    }
}

#[controller]
struct ThreeRequestFields {
    #[inject]
    label: &'static str,
    #[inject(identity)]
    user: StubIdentity,
    #[inject(request)]
    locale: Locale,
    #[inject(request)]
    attempt: Attempt,
}

#[routes]
impl ThreeRequestFields {
    #[get("/dispatch")]
    async fn handle(&self) -> StatusCode {
        black_box(self.label);
        black_box(self.user.0);
        black_box(self.locale.0);
        black_box(self.attempt.0);
        StatusCode::NO_CONTENT
    }
}

/// Everything a dispatch benchmark needs, built before counting starts.
struct Scenario {
    rt: Runtime,
    router: Routes,
    requests: Vec<Request<Body>>,
}

fn request() -> Request<Body> {
    Request::builder().uri(PATH).body(Body::empty()).unwrap()
}

fn scenario(rt: Runtime, router: Routes) -> Scenario {
    // One-shot work (lazy statics, the first poll of each service) is not
    // what a running server pays per request.
    for _ in 0..8 {
        let response = rt.block_on(router.clone().oneshot(request())).unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    let requests = (0..REQUESTS).map(|_| request()).collect();
    Scenario {
        rt,
        router,
        requests,
    }
}

fn plain_handler() -> Scenario {
    scenario(runtime(), plain_handler_router())
}

macro_rules! controller_scenario {
    ($controller:ty) => {{
        let rt = runtime();
        let router = rt.block_on(async {
            AppBuilder::new()
                .provide("ok")
                .build_state()
                .await
                .register_controller::<$controller>()
                .build()
        });
        scenario(rt, router)
    }};
}

fn app_scoped() -> Scenario {
    controller_scenario!(AppScoped)
}

fn struct_identity() -> Scenario {
    controller_scenario!(StructIdentity)
}

fn three_request_fields() -> Scenario {
    controller_scenario!(ThreeRequestFields)
}

// `REQUESTS` requests through the full `AppBuilder` stack. The scenario is
// handed back so that tearing the router and the runtime down is not counted.
#[library_benchmark]
#[bench::plain_handler(plain_handler())]
#[bench::controller(app_scoped())]
#[bench::controller_struct_identity(struct_identity())]
#[bench::controller_three_request_fields(three_request_fields())]
fn dispatch(mut scenario: Scenario) -> Scenario {
    for request in scenario.requests.drain(..) {
        let response = scenario
            .rt
            .block_on(scenario.router.clone().oneshot(request))
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        black_box(response);
    }
    scenario
}

// ── Bean graph: a chain of beans, each depending on the previous one ───────

#[derive(Clone)]
struct Root(Arc<u32>);

macro_rules! chain {
    ($previous:ident => $name:ident $(=> $rest:ident)*) => {
        #[derive(Clone)]
        #[allow(dead_code)] // nothing reads the value of the last link
        struct $name(Arc<u32>);

        #[bean]
        impl $name {
            fn new(previous: $previous) -> Self {
                Self(previous.0)
            }
        }

        chain!($name $(=> $rest)*);
    };
    ($last:ident) => {};
}

chain!(
    Root
    => C01 => C02 => C03 => C04 => C05 => C06 => C07 => C08
    => C09 => C10 => C11 => C12 => C13 => C14 => C15 => C16
    => C17 => C18 => C19 => C20 => C21 => C22 => C23 => C24
    => C25 => C26 => C27 => C28 => C29 => C30 => C31 => C32
    => C33 => C34 => C35 => C36 => C37 => C38 => C39 => C40
    => C41 => C42 => C43 => C44 => C45 => C46 => C47 => C48
    => C49 => C50 => C51 => C52 => C53 => C54 => C55 => C56
    => C57 => C58 => C59 => C60 => C61 => C62 => C63 => C64
);

/// Build the chain up to the listed beans on `$rt`, then drop the app.
macro_rules! build_chain {
    ($rt:expr; $($bean:ident)*) => {
        $rt.block_on(async {
            let app = AppBuilder::new()
                .provide(Root(Arc::new(1)))
                $(.register::<$bean>())*
                .build_state()
                .await;
            black_box(&app);
        })
    };
}

// The builds live in their own functions, not in the benchmark functions:
// Callgrind starts and stops counting on every symbol matching the benchmark
// function's name, and an `async` block written inside it is such a symbol —
// entering it would switch counting off for the very work being measured.

fn chain_of_0(rt: &Runtime) {
    build_chain!(rt;);
}

#[inline(never)]
fn chain_of_16(rt: &Runtime) {
    build_chain!(
        rt;
        C01 C02 C03 C04 C05 C06 C07 C08 C09 C10 C11 C12 C13 C14 C15 C16
    );
}

#[inline(never)]
fn chain_of_64(rt: &Runtime) {
    build_chain!(
        rt;
        C01 C02 C03 C04 C05 C06 C07 C08 C09 C10 C11 C12 C13 C14 C15 C16
        C17 C18 C19 C20 C21 C22 C23 C24 C25 C26 C27 C28 C29 C30 C31 C32
        C33 C34 C35 C36 C37 C38 C39 C40 C41 C42 C43 C44 C45 C46 C47 C48
        C49 C50 C51 C52 C53 C54 C55 C56 C57 C58 C59 C60 C61 C62 C63 C64
    );
}

/// A runtime on which one graph was already built: the first `build_state()`
/// of a process also installs the tracing subscriber, which no later build
/// pays for.
fn warmed_runtime() -> Runtime {
    let rt = runtime();
    chain_of_0(&rt);
    rt
}

// `build_state()` for a 16-bean graph. Read together with `build_64_beans`:
// the gap between the two is the marginal cost of a bean.
#[library_benchmark]
#[bench::chain(warmed_runtime())]
fn build_16_beans(rt: Runtime) -> Runtime {
    chain_of_16(&rt);
    rt
}

#[library_benchmark]
#[bench::chain(warmed_runtime())]
fn build_64_beans(rt: Runtime) -> Runtime {
    chain_of_64(&rt);
    rt
}

library_benchmark_group!(name = http, benchmarks = [dispatch]);
library_benchmark_group!(name = di, benchmarks = [build_16_beans, build_64_beans]);

main!(library_benchmark_groups = [http, di]);
