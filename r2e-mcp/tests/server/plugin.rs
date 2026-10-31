//! `McpServer` plugin behavior: enable/disable gate, endpoint path
//! precedence and validation, transport modes.

use r2e_core::http::StatusCode;
use r2e_core::{AppBuilder, R2eConfig};
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use serde_json::json;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

use crate::fixtures::{fixture_app_with, Calc, CallLog, FixtureTools};
use crate::support;

fn config(yaml: &str) -> R2eConfig {
    R2eConfig::from_yaml_str(yaml).unwrap()
}

#[r2e_core::test]
async fn disabled_plugin_mounts_nothing_but_registration_still_works() {
    // `mcp.enabled = false` drops the endpoint — but the registry is
    // deposited ungated from setup(), so `register_mcp_service` must not
    // panic.
    let router = AppBuilder::new()
        .override_config(config("mcp:\n  enabled: false\n"))
        .load_config::<()>()
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
}

/// Every member may come from `McpRegistry` after boot, so an app with no
/// `#[mcp_routes]` service still mounts the endpoint (empty lists).
#[r2e_core::test]
async fn no_registered_service_still_mounts_the_endpoint() {
    let router = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .build();

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
}

#[r2e_core::test]
async fn path_from_config_moves_the_endpoint() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .override_config(config("mcp:\n  path: /tools\n"))
        .load_config::<()>()
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(log)
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();

    let session = support::initialize(&router, "/tools").await;
    assert!(!session.is_empty());
    let miss = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn builder_path_overrides_config_path() {
    let log = CallLog::default();
    let router = AppBuilder::new()
        .override_config(config("mcp:\n  path: /from-config\n"))
        .load_config::<()>()
        .plugin(McpServer::new().with_path("/from-builder"))
        .provide(Calc)
        .provide(log)
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build();

    support::initialize(&router, "/from-builder").await;
    let miss = support::post(&router, "/from-config", None, &support::initialize_body()).await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
#[should_panic(expected = "mcp.path must start with '/'")]
async fn relative_path_is_a_boot_error() {
    let _ = fixture_app_with(McpServer::new().with_path("no-slash")).await;
}

#[r2e_core::test]
#[should_panic(expected = "without a trailing slash")]
async fn trailing_slash_path_is_a_boot_error() {
    let _ = fixture_app_with(McpServer::new().with_path("/mcp/")).await;
}

#[r2e_core::test]
#[should_panic(expected = "no `{param}` captures or wildcards")]
async fn capture_path_is_a_boot_error() {
    let _ = fixture_app_with(McpServer::new().with_path("/mcp/{id}")).await;
}

#[r2e_core::test]
async fn server_identity_is_advertised_on_initialize() {
    let (router, _log) = fixture_app_with(
        McpServer::new()
            .with_name("fixture-server")
            .with_version("9.9.9")
            .with_instructions("call add"),
    )
    .await;

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::OK);
    let result = response.result();
    assert_eq!(result["serverInfo"]["name"], "fixture-server");
    assert_eq!(result["serverInfo"]["version"], "9.9.9");
    assert_eq!(result["instructions"], "call add");
}

#[r2e_core::test]
async fn stateless_json_response_mode() {
    // `stateless` disables MCP sessions; `json-response` answers with plain
    // `application/json` instead of an SSE stream.
    let (router, _log) =
        fixture_app_with(McpServer::new().stateless(true).json_response(true)).await;

    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.raw_body);
    assert!(
        response.content_type.starts_with("application/json"),
        "expected a plain JSON response, got {}",
        response.content_type
    );
    assert!(
        response.session_id.is_none(),
        "stateless mode must not mint sessions"
    );

    // No session header needed on subsequent requests.
    let call = support::post(
        &router,
        "/mcp",
        None,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "add", "arguments": { "a": 2.0, "b": 3.0 } }
        }),
    )
    .await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.raw_body);
    assert_eq!(call.result()["structuredContent"]["value"], 5.0);
}

// ── #1185: stateless client disconnect must not log at ERROR ───────────────

mod disconnect {
    use std::sync::{Arc, Mutex};

    use http_body_util::BodyExt;
    use r2e_core::http::{Body, Request, StatusCode};
    use r2e_mcp::McpServer;
    use serde_json::json;
    use tower::ServiceExt;
    use tracing::field::{Field, Visit};
    use tracing::{Event, Level, Subscriber};
    use tracing_subscriber::layer::{Context, Layer};

    use crate::fixtures::fixture_app_with;

    /// Captures `rmcp*` events as `(level, target, message)`.
    #[derive(Clone, Default)]
    pub(super) struct Events(Arc<Mutex<Vec<(Level, String, String)>>>);

    struct Message(Option<String>);

    impl Visit for Message {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0 = Some(format!("{value:?}"));
            }
        }
    }

    impl<S: Subscriber> Layer<S> for Events {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            if !event.metadata().target().starts_with("rmcp") {
                return;
            }
            let mut message = Message(None);
            event.record(&mut message);
            self.0.lock().unwrap().push((
                *event.metadata().level(),
                event.metadata().target().to_string(),
                message.0.unwrap_or_default(),
            ));
        }
    }

    impl Events {
        pub(super) fn take(&self) -> Vec<(Level, String, String)> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    /// Let the per-request rmcp service observe the disconnect and drain.
    async fn settle() {
        for _ in 0..50 {
            r2e_core::rt::yield_now().await;
        }
        r2e_core::rt::sleep(std::time::Duration::from_millis(50)).await;
    }

    /// A 2026-07-28 `subscriptions/listen` stream on a stateless endpoint,
    /// closed by the client after the first frame — the long-lived request a
    /// client drops after its idle period. Returns the rmcp events emitted
    /// after the disconnect.
    pub(super) async fn listen_then_disconnect(events: &Events) -> Vec<(Level, String, String)> {
        let (router, _log) =
            fixture_app_with(McpServer::new().stateless(true).json_response(true)).await;
        events.take();

        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "subscriptions/listen",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {}
                },
                "notifications": { "resourceSubscriptions": ["r2e://fixture/log"] }
            }
        });
        let request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "subscriptions/listen")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let first = r2e_core::rt::timeout(std::time::Duration::from_secs(5), body.frame())
            .await
            .expect("first SSE frame within 5s")
            .expect("stream open")
            .expect("frame");
        let first = String::from_utf8_lossy(first.data_ref().expect("data frame")).to_string();
        assert!(
            first.contains("notifications/subscriptions/acknowledged"),
            "unexpected first frame: {first}"
        );
        events.take();

        drop(body);
        settle().await;
        let after = events.take();
        assert!(
            after
                .iter()
                .any(|(_, _, message)| message.contains("serve finished")),
            "the per-request service must end once the client is gone: {after:?}"
        );
        after
    }
}

/// Without a noise rule, rmcp ≤ 3.8 logs the client's disconnect at ERROR
/// (`failed to send pending response during drain`). This canary pins the
/// upstream bug: the capturing stack has no `Suppress` layer, so it sees what
/// rmcp emits. When it fails after an rmcp bump, the bug is fixed — delete
/// the entry from `r2e_mcp::RMCP_NOISE` and this test together.
#[r2e_core::test(flavor = "current_thread")]
async fn canary_rmcp_still_logs_stateless_disconnect_at_error() {
    let events = disconnect::Events::default();
    let _guard = tracing::subscriber::set_default(Registry::default().with(events.clone()));

    let after = disconnect::listen_then_disconnect(&events).await;
    assert!(
        after
            .iter()
            .any(|(level, target, message)| *level == tracing::Level::ERROR
                && r2e_mcp::RMCP_NOISE
                    .iter()
                    .any(|(t, m)| t == target && m == message)),
        "rmcp no longer logs the drain at ERROR — remove the RMCP_NOISE entry: {after:?}"
    );
}

/// `McpServer::setup` contributes `RMCP_NOISE` as `Suppress` layers, so
/// through a stack carrying them (as the framework subscriber does once
/// `build_state()` installs it) the same disconnect logs nothing at ERROR
/// (#1185).
#[r2e_core::test(flavor = "current_thread")]
async fn stateless_client_disconnect_logs_no_error() {
    let events = disconnect::Events::default();
    let suppress: Vec<_> = r2e_mcp::RMCP_NOISE
        .iter()
        .map(|(target, message)| r2e_core::Suppress::error(*target, *message))
        .collect();
    let _guard =
        tracing::subscriber::set_default(Registry::default().with(events.clone()).with(suppress));

    let after = disconnect::listen_then_disconnect(&events).await;
    let errors: Vec<_> = after
        .iter()
        .filter(|(level, _, _)| *level == tracing::Level::ERROR)
        .collect();
    assert!(
        errors.is_empty(),
        "ERROR events after a client disconnect: {errors:?}"
    );
}
