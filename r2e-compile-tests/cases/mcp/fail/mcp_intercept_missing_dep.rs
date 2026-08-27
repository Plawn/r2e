//! An `#[intercept(...)]` spec on an `#[mcp_routes]` tool reads a bean the
//! app never provided — must be rejected at `register_mcp_service()`.
//! MCP interceptor sets are prebuilt from the bean context in `tools()`,
//! and their `Deps` are folded into `EndpointDeps` exactly like HTTP route
//! and gRPC decorator deps.

use r2e::prelude::*;
use r2e::r2e_mcp::AppBuilderMcpExt;
use std::future::Future;

/// The bean the interceptor needs — deliberately never provided.
#[derive(Clone)]
pub struct AuditSink;

#[derive(DecoratorBean)]
pub struct Audit {
    #[inject]
    sink: AuditSink,
}

impl<R: Send> Interceptor<R> for Audit {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let _ = &self.sink;
        async move { next().await }
    }
}

#[controller]
pub struct PingTools {}

#[mcp_routes]
impl PingTools {
    /// Ping.
    #[tool]
    #[intercept(Audit::spec())]
    async fn ping(&self) -> String {
        unimplemented!()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_mcp_service::<PingTools>()
    };
}
