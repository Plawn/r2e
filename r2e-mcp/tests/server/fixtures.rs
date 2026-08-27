//! Shared fixtures for the `server` target: a small `#[mcp_routes]` service
//! over graph beans, a bean-reading interceptor, and a header guard —
//! the same shapes `examples/example-mcp` ships.

use std::sync::{Arc, Mutex};

use r2e_core::http::response::Response;
use r2e_core::http::Router;
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, Guard, GuardContext, Identity};
use r2e_mcp::{AppBuilderMcpExt, McpError, McpServer, Params, ToolCall};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Beans ──────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct Calc;

impl Calc {
    pub fn add(&self, a: f64, b: f64) -> f64 {
        a + b
    }

    pub fn divide(&self, a: f64, b: f64) -> Option<f64> {
        (b != 0.0).then(|| a / b)
    }
}

#[derive(Clone, Default)]
pub struct CallLog(pub Arc<Mutex<Vec<String>>>);

impl CallLog {
    pub fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

// ── Bean-reading interceptor (graph-built at registration) ────────────────

#[derive(DecoratorBean)]
pub struct LogCalls {
    #[inject]
    log: CallLog,
    tag: &'static str,
}

impl<R: Send> Interceptor<R> for LogCalls {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        async move {
            self.log
                .0
                .lock()
                .unwrap()
                .push(format!("{}:{}", self.tag, method_name));
            next().await
        }
    }
}

// ── Header guard (the SAME Guard<I> machinery HTTP routes use) ────────────

pub struct KeyGuard;

impl SelfBuilt for KeyGuard {}

impl<I: Identity> Guard<I> for KeyGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        let authorized = ctx
            .headers
            .get("x-test-key")
            .is_some_and(|v| v.as_bytes() == b"sesame");
        async move {
            if authorized {
                Ok(())
            } else {
                Err(HttpError::forbidden("missing or invalid x-test-key").into_response())
            }
        }
    }
}

// ── Tool argument / result DTOs ────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct BinaryOperands {
    /// Left operand.
    pub a: f64,
    /// Right operand.
    pub b: f64,
}

#[derive(Serialize, JsonSchema)]
pub struct CalcResult {
    /// The result of the operation.
    pub value: f64,
}

#[derive(Deserialize, JsonSchema)]
pub enum Mode {
    Fast,
    Thorough,
}

#[derive(Deserialize, JsonSchema)]
pub struct Inner {
    pub label: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct RichInput {
    /// A documented required field.
    pub name: String,
    /// An optional count.
    pub count: Option<u32>,
    pub mode: Mode,
    pub inner: Inner,
}

// ── The fixture MCP service ────────────────────────────────────────────────

#[controller]
pub struct FixtureTools {
    #[inject]
    calc: Calc,
    #[inject]
    log: CallLog,
}

#[mcp_routes]
impl FixtureTools {
    /// Add two numbers.
    #[tool(read_only, idempotent)]
    async fn add(&self, Params(p): Params<BinaryOperands>) -> Json<CalcResult> {
        Json(CalcResult {
            value: self.calc.add(p.a, p.b),
        })
    }

    /// Divide `a` by `b`.
    ///
    /// Fails with a domain error when `b` is zero.
    #[tool(name = "div", read_only)]
    #[intercept(LogCalls::spec("tool"))]
    async fn divide(
        &self,
        Params(p): Params<BinaryOperands>,
    ) -> Result<Json<CalcResult>, McpError> {
        self.calc
            .divide(p.a, p.b)
            .map(|value| Json(CalcResult { value }))
            .ok_or_else(|| McpError::tool("division by zero"))
    }

    /// Echo the JSON-RPC request id.
    #[tool]
    async fn echo_id(&self, call: ToolCall) -> String {
        format!("id={}", call.request_id)
    }

    /// Exercise the mixed typed-params + raw-call path.
    #[tool]
    async fn add_with_id(&self, Params(p): Params<BinaryOperands>, call: ToolCall) -> String {
        format!("id={};sum={}", call.request_id, self.calc.add(p.a, p.b))
    }

    /// Locked behind the `x-test-key` header.
    #[tool]
    #[guard(KeyGuard)]
    async fn locked(&self) -> String {
        let _ = &self.log;
        "unlocked".to_string()
    }

    /// Schema-rich input.
    #[tool]
    async fn rich(&self, Params(p): Params<RichInput>) -> String {
        let mode = match p.mode {
            Mode::Fast => "fast",
            Mode::Thorough => "thorough",
        };
        format!(
            "{}:{}:{}:{}",
            p.name,
            p.count.unwrap_or(0),
            mode,
            p.inner.label
        )
    }

    /// The interceptor call log, one entry per line.
    #[resource(uri = "r2e://fixture/log", mime_type = "text/plain")]
    #[intercept(LogCalls::spec("res"))]
    async fn call_log(&self) -> String {
        self.log.entries().join("\n")
    }

    /// Always fails with a domain error.
    #[resource(
        uri = "r2e://fixture/fail",
        name = "failing",
        title = "Failing resource"
    )]
    async fn failing_resource(&self) -> Result<String, McpError> {
        Err(McpError::tool("resource exploded"))
    }

    /// Explain a division.
    ///
    /// Walks the agent through dividing `a` by `b`.
    #[prompt(name = "explain_div")]
    #[intercept(LogCalls::spec("prompt"))]
    async fn explain_division(&self, Params(p): Params<BinaryOperands>) -> String {
        format!("Divide {} by {} using the `div` tool.", p.a, p.b)
    }

    /// Static usage guidance.
    #[prompt]
    async fn usage(&self) -> String {
        "Use the calculator tools for arithmetic.".to_string()
    }
}

// ── Boot helpers ────────────────────────────────────────────────────────────

/// Boot the fixture app with a configured plugin; returns the router and the
/// `CallLog` bean instance shared with the graph.
pub async fn fixture_app_with(plugin: McpServer) -> (Router, CallLog) {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .plugin(plugin)
        .provide(Calc)
        .provide(log.clone())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
    (router, log)
}

/// Boot the fixture app with plugin defaults (endpoint `/mcp`).
pub async fn fixture_app() -> (Router, CallLog) {
    fixture_app_with(McpServer::new()).await
}
