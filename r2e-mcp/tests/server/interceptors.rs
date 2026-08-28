//! Graph-built interceptors on MCP tools: `#[intercept(...)]` sites are
//! prebuilt ONCE at registration (`register_mcp_service`) from the resolved
//! bean context — the same `DecoratorSpec::build` path as HTTP route and
//! gRPC interceptors — so bean-reading specs work on tools. (The MCP peer of
//! `examples/example-grpc/tests/grpc_intercept.rs`.)

use r2e_core::http::response::Response;
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, TCons, TNil};
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::fixtures::{fixture_app, CallLog, LogCalls};
use crate::support::{initialize, tools_call};

// ── Impl-level interceptor service (single-use fixture) ────────────────────

#[controller]
pub struct WrappedTools {
    #[inject]
    log: CallLog,
}

#[mcp_routes]
#[intercept(LogCalls::spec("impl"))]
impl WrappedTools {
    /// Plain tool — wrapped only by the impl-level interceptor.
    #[tool]
    async fn plain(&self) -> String {
        let _ = &self.log;
        "plain".to_string()
    }

    /// Doubly wrapped: impl-level + method-level.
    #[tool]
    #[intercept(LogCalls::spec("method"))]
    async fn wrapped(&self) -> String {
        "wrapped".to_string()
    }
}

// A deliberately stateful impl-level interceptor. Its counter lives on the
// built product rather than in an injected bean, so calls to different tools
// only form one sequence when the product itself is genuinely shared.
pub struct SequencedCalls;

pub struct SequencedCallsReady {
    log: CallLog,
    next: AtomicUsize,
}

impl DecoratorSpec for SequencedCalls {
    type Product = SequencedCallsReady;
    type Deps = TCons<CallLog, TNil>;

    fn build(self, ctx: &r2e_core::beans::BeanContext) -> Self::Product {
        SequencedCallsReady {
            log: ctx.get::<CallLog>(),
            next: AtomicUsize::new(1),
        }
    }
}

impl<R: Send> Interceptor<R> for SequencedCallsReady {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        let sequence = self.next.fetch_add(1, Ordering::Relaxed);
        self.log
            .0
            .lock()
            .unwrap()
            .push(format!("shared:{sequence}:{}", ctx.method_name));
        next()
    }
}

pub struct TraceControllerGuard;

pub struct TraceControllerGuardReady(CallLog);

impl DecoratorSpec for TraceControllerGuard {
    type Product = TraceControllerGuardReady;
    type Deps = TCons<CallLog, TNil>;

    fn build(self, ctx: &r2e_core::beans::BeanContext) -> Self::Product {
        TraceControllerGuardReady(ctx.get::<CallLog>())
    }
}

impl Guard<NoIdentity> for TraceControllerGuardReady {
    async fn check(&self, ctx: &GuardContext<'_, NoIdentity>) -> Result<(), Response> {
        let CallLog(entries) = &self.0;
        entries
            .lock()
            .unwrap()
            .push(format!("guard:{}", ctx.method_name));
        Ok(())
    }
}

#[controller]
pub struct StatefulImplDecorators;

#[mcp_routes]
#[guard(TraceControllerGuard)]
#[intercept(SequencedCalls)]
impl StatefulImplDecorators {
    #[tool]
    async fn first(&self) -> String {
        "first".into()
    }

    #[tool]
    async fn second(&self) -> String {
        "second".into()
    }
}

#[r2e_core::test]
async fn method_level_interceptor_reads_graph_beans() {
    // The `LogCalls` spec has an `#[inject]`ed CallLog: the instance it
    // writes to MUST be the same bean instance the graph resolved — proof
    // the deco was built from the bean context at registration.
    let (router, log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    tools_call(
        &router,
        "/mcp",
        &session,
        "div",
        json!({"a": 6.0, "b": 2.0}),
    )
    .await;
    assert_eq!(log.entries(), vec!["tool:divide"]);

    // Un-intercepted tools do not log.
    tools_call(
        &router,
        "/mcp",
        &session,
        "add",
        json!({"a": 1.0, "b": 1.0}),
    )
    .await;
    assert_eq!(log.entries(), vec!["tool:divide"]);

    // The wrapper is reused across calls (entries accumulate on the same
    // instance).
    tools_call(
        &router,
        "/mcp",
        &session,
        "div",
        json!({"a": 1.0, "b": 1.0}),
    )
    .await;
    assert_eq!(log.entries(), vec!["tool:divide", "tool:divide"]);
}

#[r2e_core::test]
async fn impl_level_interceptor_wraps_every_tool() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .provide(log.clone())
        .build_state()
        .await
        .register_mcp_service::<WrappedTools>()
        .build();
    let session = initialize(&router, "/mcp").await;

    tools_call(&router, "/mcp", &session, "plain", json!({})).await;
    assert_eq!(log.entries(), vec!["impl:plain"]);

    // Method-level wraps inside the impl-level set: both fire, impl first.
    tools_call(&router, "/mcp", &session, "wrapped", json!({})).await;
    assert_eq!(
        log.entries(),
        vec!["impl:plain", "impl:wrapped", "method:wrapped"]
    );
}

#[r2e_core::test]
async fn impl_level_decorators_are_one_shared_stateful_instance() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .provide(log.clone())
        .build_state()
        .await
        .register_mcp_service::<StatefulImplDecorators>()
        .build();
    let session = initialize(&router, "/mcp").await;

    tools_call(&router, "/mcp", &session, "first", json!({})).await;
    tools_call(&router, "/mcp", &session, "second", json!({})).await;
    tools_call(&router, "/mcp", &session, "first", json!({})).await;

    assert_eq!(
        log.entries(),
        vec![
            "guard:*",
            "shared:1:first",
            "guard:*",
            "shared:2:second",
            "guard:*",
            "shared:3:first",
        ]
    );
}

#[r2e_core::test]
async fn interceptor_wraps_the_error_path_too() {
    let (router, log) = fixture_app().await;
    let session = initialize(&router, "/mcp").await;

    let msg = tools_call(
        &router,
        "/mcp",
        &session,
        "div",
        json!({"a": 1.0, "b": 0.0}),
    )
    .await;
    assert_eq!(msg["result"]["isError"], true);
    assert_eq!(log.entries(), vec!["tool:divide"]);
}
