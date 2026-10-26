//! Where `/mcp` is mounted: in the Routes stage, INSIDE the app layer stack.
//!
//! Every Graph-stage `add_layer` (a plain middleware, `HttpTrace`, an app
//! CORS layer) must wrap the MCP endpoint — and, with `mcp.auth`, the public
//! well-known routes — exactly like a controller route. A Finalize-stage
//! mount (`wrap_router`) would sit outside all of them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use r2e_core::builtins::Cors;
use r2e_core::http::header::{HeaderName, AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use r2e_core::http::layers::cors::{AllowOrigin, CorsLayer};
use r2e_core::http::middleware::Next;
use r2e_core::http::{Body, HeaderMap, Method, Request, Routes, StatusCode};
use r2e_core::plugin::{PluginBuildContext, PluginBuildError};
use r2e_core::{AppBuilder, HttpTrace, Plugin, R2eConfig};
use r2e_mcp::auth::{DiscoveryMode, McpAuthConfig, ScopePolicy};
use r2e_mcp::{AppBuilderMcpExt, McpServer, McpTokenValidator};
use r2e_test::TestJwt;
use serde_json::json;
use tower::ServiceExt;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::Registry;

use crate::fixtures::{Calc, CallLog, FixtureTools};
use crate::support;

// ── A Graph-stage counting middleware ──────────────────────────────────────

/// Records `"<METHOD> <path>"` for every request it sees.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<String>>>);

impl Seen {
    fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    fn count(&self, entry: &str) -> usize {
        self.entries().iter().filter(|e| *e == entry).count()
    }
}

/// A plugin whose only effect is a Graph-stage `add_layer` middleware.
struct CountRequests(Seen);

impl Plugin for CountRequests {
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
        let seen = self.0;
        ctx.add_layer(move |router| {
            router.middleware_fn(move |req: Request, next: Next| {
                let seen = seen.clone();
                async move {
                    seen.0
                        .lock()
                        .unwrap()
                        .push(format!("{} {}", req.method(), req.uri().path()));
                    next.run(req).await
                }
            })
        });
        Ok(())
    }
}

/// Boot the fixture service under `CountRequests`, with `mcp` as the plugin.
async fn counted_app(mcp: McpServer) -> (Routes, Seen) {
    let seen = Seen::default();
    let router = AppBuilder::new()
        .plugin(CountRequests(seen.clone()))
        .plugin(mcp)
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
    (router, seen)
}

/// Plain request helper (well-known GETs, preflights, the SSE GET).
async fn send(
    router: &Routes,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    // The body is dropped unread: an SSE stream never ends, and every
    // assertion here is on the response head.
    (response.status(), response.headers().clone())
}

#[r2e_core::test]
async fn graph_layer_sees_mcp_requests() {
    let (router, seen) = counted_app(McpServer::new()).await;

    let session = support::initialize(&router, "/mcp").await;
    let reply = support::tools_call(
        &router,
        "/mcp",
        &session,
        "add",
        json!({ "a": 2.0, "b": 3.0 }),
    )
    .await;
    assert_eq!(
        reply["result"]["structuredContent"]["value"], 5.0,
        "{reply}"
    );

    // initialize + notifications/initialized + tools/call.
    assert_eq!(seen.count("POST /mcp"), 3, "{:?}", seen.entries());
}

// ── With `mcp.auth`: the well-known routes cross the app layers too ────────

const ISSUER: &str = "http://idp.test";
const RESOURCE: &str = "http://localhost:3000/mcp";
const PRM_SUFFIXED: &str = "/.well-known/oauth-protected-resource/mcp";

/// Offline auth: `discovery: off`, dead JWKS URL, validator pinned to a
/// TestJwt secret — zero network I/O.
fn offline_auth_plugin() -> McpServer {
    let jwt = TestJwt::with_config(b"mount-target-secret", ISSUER, RESOURCE);
    McpServer::new()
        .with_auth(McpAuthConfig {
            issuer: ISSUER.to_string(),
            resource: Some(RESOURCE.to_string()),
            discovery: Some(DiscoveryMode::Off),
            jwks_url: Some("http://127.0.0.1:1/jwks".to_string()),
            allow_insecure: Some(true),
            ..Default::default()
        })
        .with_token_validator(McpTokenValidator::jwt(
            Arc::new(jwt.claims_validator()),
            ScopePolicy::default(),
        ))
}

#[r2e_core::test]
async fn graph_layer_sees_well_known_routes_and_they_stay_public() {
    let (router, seen) = counted_app(offline_auth_plugin()).await;

    // No bearer token: the PRM document is what a client fetches BEFORE it
    // has one — public, though it now crosses every app Graph layer.
    let (status, _) = send(&router, "GET", PRM_SUFFIXED, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        seen.count(&format!("GET {PRM_SUFFIXED}")),
        1,
        "{:?}",
        seen.entries()
    );

    // The MCP endpoint itself still challenges — and the challenge also
    // crossed the app layer.
    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(seen.count("POST /mcp"), 1, "{:?}", seen.entries());
}

// ── HttpTrace: one summary line per MCP request ────────────────────────────

/// Collects the `status` of every `request completed` summary event.
#[derive(Clone, Default)]
struct Summaries(Arc<Mutex<Vec<String>>>);

struct Fields(HashMap<String, String>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl<S: Subscriber> Layer<S> for Summaries {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields(HashMap::new());
        event.record(&mut fields);
        if fields.0.get("message").map(String::as_str) == Some("request completed") {
            let status = fields.0.remove("status").unwrap_or_default();
            self.0.lock().unwrap().push(status);
        }
    }
}

impl Summaries {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// Thread-local subscriber on a `current_thread` runtime (the r2e-core
/// `http_trace` idiom): the whole request is polled on the test thread, so
/// the capture cannot be interleaved by the other tests of this binary.
#[r2e_core::test(flavor = "current_thread")]
async fn http_trace_logs_one_summary_per_mcp_request() {
    let summaries = Summaries::default();
    let _guard = tracing::subscriber::set_default(Registry::default().with(summaries.clone()));

    let router = AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str("server:\n  port: 0\n").unwrap())
        .load_config::<()>()
        .plugin(HttpTrace::new())
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();
    summaries.take();

    let init = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(init.status, StatusCode::OK, "{}", init.raw_body);
    assert_eq!(summaries.take(), ["200"], "POST initialize");
    let session = init.session_id.clone().expect("Mcp-Session-Id");

    let notified = support::post(
        &router,
        "/mcp",
        Some(&session),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(notified.status, StatusCode::ACCEPTED);
    assert_eq!(summaries.take(), ["202"], "POST notifications/initialized");

    let reply = support::tools_call(
        &router,
        "/mcp",
        &session,
        "add",
        json!({ "a": 1.0, "b": 1.0 }),
    )
    .await;
    assert!(reply.get("result").is_some(), "{reply}");
    assert_eq!(summaries.take(), ["200"], "POST tools/call");

    // The standalone SSE stream: summarised once, at the response head.
    let (status, _) = send(
        &router,
        "GET",
        "/mcp",
        &[
            ("accept", "text/event-stream"),
            ("mcp-session-id", &session),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(summaries.take(), ["200"], "GET /mcp (SSE)");
}

// ── App CORS layer + MCP CORS layer ────────────────────────────────────────

/// The app-level CORS layer an MCP app must install when it adds its own:
/// it answers preflights before the MCP layer sees them, so it carries the
/// MCP header lists (see the `McpServer` "Double CORS" docs).
fn app_cors() -> CorsLayer {
    let session_id = HeaderName::from_static("mcp-session-id");
    CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            r2e_core::http::HeaderValue::from_static("https://claude.ai"),
        ]))
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            HeaderName::from_static("mcp-protocol-version"),
            session_id.clone(),
            HeaderName::from_static("last-event-id"),
        ])
        .expose_headers([WWW_AUTHENTICATE, session_id])
}

#[r2e_core::test]
async fn app_cors_answers_the_preflight_and_mcp_still_works() {
    let router = AppBuilder::new()
        .plugin(Cors::custom(app_cors()))
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();

    let (status, headers) = send(
        &router,
        "OPTIONS",
        "/mcp",
        &[
            ("origin", "https://claude.ai"),
            ("access-control-request-method", "POST"),
            (
                "access-control-request-headers",
                "content-type, mcp-session-id, mcp-protocol-version",
            ),
        ],
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::NO_CONTENT,
        "preflight status {status}"
    );
    let origins: Vec<_> = headers
        .get_all("access-control-allow-origin")
        .iter()
        .collect();
    assert_eq!(origins, ["https://claude.ai"], "answered exactly once");
    let allowed = headers
        .get("access-control-allow-headers")
        .map(|v| v.to_str().unwrap().to_ascii_lowercase())
        .unwrap_or_default();
    for name in ["mcp-session-id", "mcp-protocol-version", "content-type"] {
        assert!(allowed.contains(name), "{name} not allowed: {allowed}");
    }

    // A real cross-origin initialize goes through both CORS layers.
    let response = support::post_with_headers(
        &router,
        "/mcp",
        None,
        &[("origin", "https://claude.ai")],
        &support::initialize_body(),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    assert!(response.session_id.is_some());
}
