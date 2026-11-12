//! The request hot path: what a controller costs over a hand-written handler,
//! and what the application stack costs over a bare router.

use std::hint::black_box;
use std::sync::Arc;

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::response::Response;
use r2e_core::http::routing::get;
use r2e_core::http::{Body, Request, Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::AppBuilder;

use crate::fixtures::{allocations_per_request, StubIdentity, PATH};

/// Allocations a controller may add per request over a hand-written handler
/// registered on the same `AppBuilder`. The generated handler clones one
/// `Arc` and extracts in place: it has no reason to reach the allocator.
const CONTROLLER_OVERHEAD: u64 = 0;

/// Allocations the default application stack (framework 404/405, the two
/// catch-panic slots, the graph keep-alive) may add per request over a bare
/// router. **Absolute**: it moves when a default layer is added or removed,
/// and may move with an axum/tower upgrade — re-measure, don't guess.
const APP_STACK_OVERHEAD: u64 = 13;

#[derive(Clone)]
struct PlainState;

struct PlainCore {
    label: &'static str,
}

/// The hand-written equivalent of `AppScoped`: a captured `Arc` core.
fn plain_routes() -> Routes<PlainState> {
    let core = Arc::new(PlainCore { label: "ok" });
    Routes::new().route(
        PATH,
        get(move |_: PlainState, _| async move {
            black_box(core.label);
            StatusCode::NO_CONTENT
        }),
    )
}

/// The hand-written equivalent of the identity controllers.
fn plain_identity_routes() -> Routes<PlainState> {
    let core = Arc::new(PlainCore { label: "ok" });
    Routes::new().route(
        PATH,
        get(move |_: PlainState, request: Request<Body>| async move {
            let (mut parts, _) = request.into_parts();
            let user = StubIdentity::from_request_parts(&mut parts, &()).await?;
            black_box((core.label, user.0));
            Ok::<_, Response>(StatusCode::NO_CONTENT)
        }),
    )
}

fn on_app_stack(routes: Routes<PlainState>) -> Routes {
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
    #[get("/budget")]
    async fn handle(&self) -> StatusCode {
        black_box(self.label);
        StatusCode::NO_CONTENT
    }
}

#[controller]
struct ParamIdentity {
    #[inject]
    label: &'static str,
}

#[routes]
impl ParamIdentity {
    #[get("/budget")]
    async fn handle(&self, #[inject(identity)] user: StubIdentity) -> StatusCode {
        black_box((self.label, user.0));
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
    #[get("/budget")]
    async fn handle(&self) -> StatusCode {
        black_box((self.label, self.user.0));
        StatusCode::NO_CONTENT
    }
}

macro_rules! controller_router {
    ($controller:ty) => {
        AppBuilder::new()
            .provide("ok")
            .build_state()
            .await
            .register_controller::<$controller>()
            .build()
    };
}

#[r2e_core::test(flavor = "current_thread")]
async fn app_scoped_controller_costs_no_more_than_a_plain_handler() {
    let plain = allocations_per_request(&on_app_stack(plain_routes())).await;
    let controller = allocations_per_request(&controller_router!(AppScoped)).await;

    assert!(
        controller <= plain + CONTROLLER_OVERHEAD,
        "a controller with app-scoped fields allocates {controller} times per request, \
         a hand-written handler on the same stack {plain} (budget: +{CONTROLLER_OVERHEAD})"
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn identity_controllers_cost_no_more_than_a_plain_handler() {
    let plain = allocations_per_request(&on_app_stack(plain_identity_routes())).await;
    let param = allocations_per_request(&controller_router!(ParamIdentity)).await;
    let on_struct = allocations_per_request(&controller_router!(StructIdentity)).await;

    assert!(
        param <= plain + CONTROLLER_OVERHEAD,
        "parameter-level identity allocates {param} times per request, \
         a hand-written handler on the same stack {plain} (budget: +{CONTROLLER_OVERHEAD})"
    );
    assert!(
        on_struct <= plain + CONTROLLER_OVERHEAD,
        "struct-level identity allocates {on_struct} times per request, \
         a hand-written handler on the same stack {plain} (budget: +{CONTROLLER_OVERHEAD})"
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn app_stack_stays_within_its_budget_over_a_bare_router() {
    let bare = allocations_per_request(&plain_routes().with_state(PlainState)).await;
    let stacked = allocations_per_request(&on_app_stack(plain_routes())).await;

    assert!(
        stacked <= bare + APP_STACK_OVERHEAD,
        "the default application stack allocates {stacked} times per request, \
         a bare router {bare} (budget: +{APP_STACK_OVERHEAD})"
    );
}
