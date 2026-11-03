//! Task #1212 — the decorator storage layout costs nothing per call.
//!
//! Guard and interceptor products are built once at registration and live
//! beside the core in the single `Arc`'d service wrapper. Invoking a member
//! borrows them in place: no `Arc` clone of a decorator set and no allocation
//! per site. The guard is a delta: a member with controller-level and
//! member-level guards and interceptors costs exactly as many allocations per
//! call as the same member with none (the boxed invoke future grows with the
//! number of sites, so bytes are reported, not compared).

use std::sync::Arc;

use r2e_core::beans::BeanContext;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{McpService, ToolCall};
use serde_json::json;

use crate::counter::{runtime, steady_state, Alloc};

// ── No-op decorators: whatever they allocate is not the layout's cost ──────

pub struct Pass;

impl SelfBuilt for Pass {}

impl<I: Identity> Guard<I> for Pass {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Rejection>> + Send {
        async { Ok(()) }
    }
}

pub struct Through;

impl SelfBuilt for Through {}

impl<R: Send> Interceptor<R> for Through {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        next()
    }
}

// ── Fixture services ───────────────────────────────────────────────────────

#[controller]
struct Plain;

#[mcp_routes]
impl Plain {
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[controller]
struct Decorated;

#[mcp_routes]
#[guard(Pass)]
#[intercept(Through)]
impl Decorated {
    #[tool]
    #[guard(Pass)]
    #[guard(Pass)]
    #[intercept(Through)]
    #[intercept(Through)]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

fn context(rt: &r2e_core::rt::Runtime) -> Arc<BeanContext> {
    rt.block_on(async { AppBuilder::new().build_state().await.bean_context().clone() })
}

/// Per-call cost of invoking the service's only tool directly through its
/// route (no transport: the measurement is the generated invoke path).
fn cost_per_call<S: McpService<(), W>, W>(rt: &r2e_core::rt::Runtime) -> Alloc {
    let ctx = context(rt);
    let routes = S::routes(&(), &ctx);
    assert_eq!(routes.tools.len(), 1);
    let invoke = routes.tools[0].invoke.clone();
    steady_state(50, || {
        let result = rt
            .block_on(invoke(ToolCall::new(json!({}))))
            .expect("tool call succeeds");
        drop(result);
    })
}

/// Wall-clock per call, for the record only (never asserted: CI timing is
/// noise).
fn time_per_call<S: McpService<(), W>, W>(rt: &r2e_core::rt::Runtime) -> std::time::Duration {
    const N: u32 = 20_000;
    let ctx = context(rt);
    let routes = S::routes(&(), &ctx);
    let invoke = routes.tools[0].invoke.clone();
    let start = std::time::Instant::now();
    rt.block_on(async {
        for _ in 0..N {
            drop(invoke(ToolCall::new(json!({}))).await);
        }
    });
    start.elapsed() / N
}

#[test]
fn decorated_members_cost_no_extra_allocation_per_call() {
    let rt = runtime();
    let plain = cost_per_call::<Plain, _>(&rt);
    let decorated = cost_per_call::<Decorated, _>(&rt);
    eprintln!(
        "[hotpath] member call: undecorated = {plain}; 2 ctrl + 4 member sites = {decorated}"
    );
    eprintln!(
        "[hotpath] member call time: undecorated = {:?}; decorated = {:?} (per call, informative)",
        time_per_call::<Plain, _>(&rt),
        time_per_call::<Decorated, _>(&rt),
    );
    assert_eq!(
        decorated.count, plain.count,
        "a guarded + intercepted member allocates more per call than a bare one — the \
         decorator products are being cloned or boxed per call instead of borrowed from \
         the service wrapper",
    );
}
