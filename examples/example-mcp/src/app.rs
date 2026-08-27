// Canonical example-mcp application source.
//
// `lib.rs` includes this file so the app can be booted by type (integration
// tests use `#[r2e::test(app = McpApp)]`); `app_main!` includes the same file
// directly in the binary tip crate for production and real Subsecond
// hot-patching.

use std::sync::{Arc, Mutex};

use r2e::prelude::*;
use r2e::{Guard, GuardContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Shared domain service ──────────────────────────────────────────────
//
// ONE bean serves BOTH transports. `EndpointDeps` is one-per-type, so a
// single struct cannot carry `#[routes]` and `#[mcp_routes]` at once — the
// pattern is a shared bean plus two thin adapters (`MathTools` for MCP,
// `CalcController` for HTTP below).

#[derive(Clone, Default)]
pub struct CalcService;

impl CalcService {
    pub fn add(&self, a: f64, b: f64) -> f64 {
        a + b
    }

    pub fn divide(&self, a: f64, b: f64) -> Option<f64> {
        (b != 0.0).then(|| a / b)
    }
}

/// Bean read by the call-log interceptor and the `clear_log` tool.
#[derive(Clone, Default)]
pub struct CallLog(pub Arc<Mutex<Vec<String>>>);

// ── Interceptor built from the bean graph ──────────────────────────────
//
// MCP `#[intercept(...)]` sites are prebuilt once at registration
// (`register_mcp_service`), from the resolved bean context — the same
// `DecoratorSpec` path as HTTP route and gRPC interceptors, so bean-reading
// specs work here too.

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

// ── Guard shared with HTTP ─────────────────────────────────────────────
//
// MCP tools reuse the SAME `Guard<I>` machinery as HTTP routes: the
// streamable-HTTP transport hands each tool call its originating request
// parts, so this guard reads a real header. Rejections are folded back into
// JSON-RPC errors by status (403 → Forbidden).

pub struct ApiKeyGuard;

impl r2e::SelfBuilt for ApiKeyGuard {}

impl<I: Identity> Guard<I> for ApiKeyGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        let authorized = ctx
            .headers
            .get("x-api-key")
            .is_some_and(|v| v.as_bytes() == b"letmein");
        async move {
            if authorized {
                Ok(())
            } else {
                Err(HttpError::forbidden("missing or invalid x-api-key").into_response())
            }
        }
    }
}

// ── Tool argument / result DTOs ────────────────────────────────────────
//
// Arguments derive `Deserialize + JsonSchema` (the schema becomes the tool's
// `inputSchema`; doc comments become property descriptions). A `Json<T>`
// return with `T: Serialize + JsonSchema` additionally advertises an
// `outputSchema` and lands in `structuredContent`.

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

// ── MCP service ────────────────────────────────────────────────────────

#[controller]
pub struct MathTools {
    #[inject]
    calc: CalcService,
    #[inject]
    log: CallLog,
}

#[mcp_routes]
#[intercept(Logged::info())]
impl MathTools {
    /// Add two numbers and return their sum.
    #[tool(read_only, idempotent)]
    async fn add(&self, Params(p): Params<BinaryOperands>) -> Json<CalcResult> {
        Json(CalcResult {
            value: self.calc.add(p.a, p.b),
        })
    }

    /// Divide `a` by `b`.
    ///
    /// Fails with a domain error (readable by the calling agent) when `b`
    /// is zero.
    #[tool(name = "divide", read_only)]
    #[intercept(LogCalls::spec("tool"))]
    async fn divide(&self, Params(p): Params<BinaryOperands>) -> Result<Json<CalcResult>, McpError> {
        self.calc
            .divide(p.a, p.b)
            .map(|value| Json(CalcResult { value }))
            .ok_or_else(|| McpError::tool("division by zero: pass a non-zero `b`"))
    }

    /// Report the calls the interceptor has logged so far.
    #[tool(read_only)]
    async fn call_log(&self) -> String {
        self.log.0.lock().unwrap().join("\n")
    }

    /// Clear the call log. Requires the `x-api-key` header on the HTTP
    /// request carrying the tool call.
    #[tool(destructive)]
    #[guard(ApiKeyGuard)]
    async fn clear_log(&self, call: ToolCall) -> String {
        let mut log = self.log.0.lock().unwrap();
        let cleared = log.len();
        log.clear();
        format!("cleared {cleared} entries (request {})", call.request_id)
    }

    /// The call log as a fixed-URI MCP resource — same data as the
    /// `call_log` tool, but readable via `resources/read` (agents can
    /// subscribe it into context instead of calling a tool).
    #[resource(uri = "r2e://calc/call-log", mime_type = "text/plain")]
    async fn call_log_resource(&self) -> String {
        self.log.0.lock().unwrap().join("\n")
    }

    /// Reusable prompt template guiding an agent through a division,
    /// including the division-by-zero contract. Arguments are derived from
    /// the `Params` schema and advertised in `prompts/list`.
    #[prompt(name = "explain_division")]
    async fn explain_division(&self, Params(p): Params<BinaryOperands>) -> String {
        format!(
            "Divide {} by {} using the `divide` tool. If the divisor is zero, \
             report the tool's domain error to the user instead of retrying.",
            p.a, p.b
        )
    }
}

// ── HTTP controller over the same bean ─────────────────────────────────

#[controller(path = "/api/calc")]
pub struct CalcController {
    #[inject]
    calc: CalcService,
}

#[routes]
impl CalcController {
    #[get("/add/{a}/{b}")]
    async fn add(&self, Path((a, b)): Path<(f64, f64)>) -> Json<CalcResult> {
        Json(CalcResult {
            value: self.calc.add(a, b),
        })
    }
}

// ── Application blueprint ──────────────────────────────────────────────

/// The canonical application blueprint. Serves HTTP on :3000 (the
/// `serve_auto` default that `launch` uses) with the MCP endpoint mounted at
/// `/mcp` — point `npx @modelcontextprotocol/inspector` at
/// `http://localhost:3000/mcp`.
pub struct McpApp;

impl App for McpApp {
    type Env = ();

    async fn setup() {}

    async fn build(b: AppBuilder, _env: ()) -> impl BootableApp {
        // Raw config load: honors `application.yaml` (none here), the profile
        // overlay and `R2E_*` env vars — e.g. `R2E_MCP_PATH=/tools` or
        // `R2E_SERVER_WORKERS=4` (SO_REUSEPORT sharded serving; the MCP
        // session map is shared across workers).
        b.load_config::<()>()
            .plugin(
                McpServer::new()
                    .with_name("example-mcp")
                    .with_instructions("Calculator tools: add, divide, call_log, clear_log."),
            )
            .provide(CalcService)
            .provide(CallLog::default())
            .build_state()
            .await
            .on_start(|_state| async move {
                tracing::info!("HTTP on :3000, MCP endpoint at /mcp");
                Ok(())
            })
            .register_mcp_service::<MathTools>()
            .register_controller::<CalcController>()
    }
}
