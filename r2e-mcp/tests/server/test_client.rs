//! `r2e_mcp::testing::McpTestClient` — feature `testing`. `app.mcp()` on a
//! `TestApp` opens a real loopback session on the mounted endpoint:
//!
//! * anonymous by default; `as_user` mints the `TestJwt` bearer HTTP's
//!   `as_user` sends (roles, `with_scopes`), validated by the pinned
//!   validator; refusals surface through `try_connect`;
//! * typed helpers: `call_tool::<Out>` decodes `structuredContent` or the
//!   first text block, `call_tool_text`, `call_tool_raw`; resources and
//!   prompts; JSON-RPC errors and `isError` results as `McpTestError`;
//! * server notifications are captured: list-changed, resource updates,
//!   progress.

use std::time::Duration;

use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::testing::{pin_mcp_validator, McpNotification, McpTestError, McpTestExt};
use r2e_mcp::{
    AppBuilderMcpExt, LoopbackError, McpError, McpPrincipal, McpResourceUpdates, McpServer,
    McpSessions, Params, ToolCall,
};
use r2e_test::{TestApp, TestJwt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::fixtures::{Calc, CallLog, FixtureTools};
use crate::progress::ProgressTools;

// ── Fixtures ───────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema, ObjectParams)]
struct Operands {
    a: i64,
    b: i64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Sum {
    total: i64,
}

#[controller]
struct ClientTools;

#[routes]
impl ClientTools {
    /// Structured output.
    #[tool]
    async fn sum(&self, Params(p): Params<Operands>) -> Json<Sum> {
        Json(Sum { total: p.a + p.b })
    }

    /// Plain-text output that is not JSON.
    #[tool]
    async fn label(&self) -> &'static str {
        "not json"
    }

    /// Plain-text output that happens to be JSON.
    #[tool]
    async fn count(&self) -> String {
        "42".to_string()
    }

    /// A domain failure: an `isError: true` result.
    #[tool]
    async fn boom(&self) -> Result<String, McpError> {
        Err(McpError::tool("it exploded"))
    }

    /// The transport session id as the tool sees it.
    #[tool]
    async fn session(&self, call: ToolCall) -> String {
        call.session_id()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "<none>".to_string())
    }

    /// The caller, if any.
    #[tool]
    async fn whoami(&self, #[inject(identity)] principal: Option<McpPrincipal>) -> String {
        principal
            .map(|p| p.user.sub.clone())
            .unwrap_or_else(|| "<anonymous>".to_string())
    }
}

#[controller]
struct SecuredClientTools;

#[routes]
impl SecuredClientTools {
    #[tool]
    async fn whoami(&self, #[inject(identity)] principal: McpPrincipal) -> String {
        principal.user.sub.clone()
    }

    #[tool(scopes = "mcp:read")]
    async fn read_data(&self) -> &'static str {
        "data"
    }

    #[tool]
    #[roles("admin")]
    async fn admin_only(&self, #[inject(identity)] principal: McpPrincipal) -> String {
        format!("admin:{}", principal.user.sub)
    }
}

const RESOURCE: &str = "http://localhost:3000/mcp";

/// `mcp.auth` off: every fixture service on one endpoint.
async fn plain_app() -> TestApp {
    let builder = AppBuilder::new()
        .plugin(McpServer::new())
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<ClientTools>()
        .register_mcp_service::<FixtureTools>()
        .register_mcp_service::<ProgressTools>();
    TestApp::from_builder(builder)
}

/// `mcp.auth` on, validator pinned to a `TestJwt` the app carries.
async fn secured_app() -> TestApp {
    let jwt = TestJwt::for_resource(RESOURCE);
    let builder = pin_mcp_validator(AppBuilder::new(), &jwt, RESOURCE)
        .load_config::<()>()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<SecuredClientTools>();
    TestApp::from_builder(builder).with_jwt(jwt)
}

// ── Connecting ─────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn the_default_client_is_an_anonymous_real_session() {
    let app = plain_app().await;
    let mcp = app.mcp().await;

    assert_eq!(
        mcp.call_tool_text("whoami", ()).await.unwrap(),
        "<anonymous>"
    );
    // A real session: the tool sees its id, `McpSessions` holds it.
    assert_eq!(
        mcp.call_tool_text("session", ()).await.unwrap(),
        mcp.session_id()
    );
    let sessions = app.bean::<McpSessions>();
    assert!(sessions.get(mcp.session_id()).is_some());

    // Two clients never collide on the generated id.
    let other = app.mcp().await;
    assert_ne!(other.session_id(), mcp.session_id());

    let id = mcp.session_id().to_owned();
    mcp.close().await;
    assert!(sessions.get(&id).is_none());

    let named = app.mcp().session_id("mine").await;
    assert_eq!(named.call_tool_text("session", ()).await.unwrap(), "mine");
}

#[r2e_core::test]
async fn as_user_sends_the_test_jwt_bearer() {
    let app = secured_app().await;

    let alice = app
        .mcp()
        .as_user("alice", &["admin"])
        .with_scopes(&["mcp:read"])
        .await;
    assert_eq!(alice.call_tool_text("whoami", ()).await.unwrap(), "alice");
    assert_eq!(
        alice.tool_names().await,
        ["whoami", "read_data", "admin_only"]
    );
    assert_eq!(
        alice.call_tool_text("admin_only", ()).await.unwrap(),
        "admin:alice"
    );
    assert_eq!(alice.call_tool_text("read_data", ()).await.unwrap(), "data");

    // No role, no scope: filtered out of the list, refused on call.
    let bob = app.mcp().as_user("bob", &[]).await;
    assert_eq!(bob.tool_names().await, ["whoami"]);
    let err = bob.call_tool_text("admin_only", ()).await.unwrap_err();
    assert!(err.code().is_some(), "a JSON-RPC refusal, got {err:?}");
    assert!(bob.call_tool_text("read_data", ()).await.is_err());
}

/// `TestApp::boot` wires the app-wide `TestJwt::new()`, whose audience is not
/// the MCP resource: `as_user` binds the token to the endpoint's resource.
#[r2e_core::test]
async fn as_user_binds_the_token_to_the_endpoint_resource() {
    let builder = pin_mcp_validator(
        AppBuilder::new(),
        &TestJwt::for_resource(RESOURCE),
        RESOURCE,
    )
    .load_config::<()>()
    .plugin(McpServer::new())
    .build_state()
    .await
    .register_mcp_service::<SecuredClientTools>();
    let app_jwt = TestJwt::new();
    assert_ne!(app_jwt.audience(), RESOURCE);
    let app = TestApp::from_builder(builder).with_jwt(app_jwt.clone());

    let alice = app.mcp().as_user("alice", &[]).await;
    assert_eq!(alice.call_tool_text("whoami", ()).await.unwrap(), "alice");

    // The app-wide audience alone is still refused: the binding is what
    // lets the token through, not a relaxed validator.
    let refused = app
        .mcp()
        .bearer(app_jwt.token("alice", &[]))
        .try_connect()
        .await;
    assert!(refused.is_err(), "wrong-audience token must be refused");
}

#[r2e_core::test]
async fn refused_connections_surface_through_try_connect() {
    let app = secured_app().await;

    // Fail-closed: anonymous under `mcp.auth`.
    let err = app.mcp().try_connect().await.unwrap_err();
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");

    let err = app
        .mcp()
        .bearer(TestJwt::malformed_token())
        .try_connect()
        .await
        .unwrap_err();
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");

    let err = app
        .mcp()
        .bearer(app.test_jwt().wrong_audience_token("eve"))
        .try_connect()
        .await
        .unwrap_err();
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");

    // A raw bearer that is valid connects.
    let token = app.test_jwt().token("carol", &[]);
    let carol = app.mcp().bearer(token).await;
    assert_eq!(carol.call_tool_text("whoami", ()).await.unwrap(), "carol");
}

#[r2e_core::test]
async fn as_principal_bypasses_token_validation() {
    let app = plain_app().await;
    let principal = McpPrincipal::trusted(
        r2e_security::AuthenticatedUser::from_claims(r2e_security::StandardClaims {
            sub: "dave".to_string(),
            ..Default::default()
        }),
        Vec::<String>::new(),
    );
    let dave = app.mcp().as_principal(principal).await;
    assert_eq!(dave.call_tool_text("whoami", ()).await.unwrap(), "dave");
}

// ── Tools ──────────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn call_tool_decodes_structured_and_text_results() {
    let app = plain_app().await;
    let mcp = app.mcp().await;

    // structuredContent, into a typed struct or a dynamic value.
    let sum: Sum = mcp.call_tool("sum", json!({ "a": 2, "b": 3 })).await.unwrap();
    assert_eq!(sum, Sum { total: 5 });
    let value: serde_json::Value = mcp
        .call_tool("sum", json!({ "a": 1, "b": 1 }))
        .await
        .unwrap();
    assert_eq!(value, json!({ "total": 2 }));

    // Arguments from a `Serialize` struct.
    #[derive(Serialize)]
    struct Args {
        a: i64,
        b: i64,
    }
    let sum: Sum = mcp.call_tool("sum", Args { a: 20, b: 22 }).await.unwrap();
    assert_eq!(sum.total, 42);

    // A text block: JSON when it parses, the string itself otherwise.
    let count: u32 = mcp.call_tool("count", ()).await.unwrap();
    assert_eq!(count, 42);
    let label: String = mcp.call_tool("label", ()).await.unwrap();
    assert_eq!(label, "not json");
    assert_eq!(mcp.call_tool_text("label", ()).await.unwrap(), "not json");

    // Neither shape fits the requested type.
    let err = mcp.call_tool::<Sum>("label", ()).await.unwrap_err();
    assert!(matches!(err, McpTestError::Decode { .. }), "{err:?}");
}

#[r2e_core::test]
async fn failures_are_results_not_panics() {
    let app = plain_app().await;
    let mcp = app.mcp().await;

    // The tool ran and failed: `isError: true`.
    let err = mcp.call_tool::<String>("boom", ()).await.unwrap_err();
    assert!(matches!(err, McpTestError::Tool(_)), "{err:?}");
    assert_eq!(err.tool_error_text().as_deref(), Some("it exploded"));
    assert_eq!(err.code(), None);
    assert!(err.to_string().contains("it exploded"), "{err}");

    // ... still reachable untouched.
    let raw = mcp.call_tool_raw("boom", ()).await.unwrap();
    assert_eq!(raw.is_error, Some(true));

    // Protocol-level refusals: unknown tool, invalid params.
    let err = mcp.call_tool_text("nope", ()).await.unwrap_err();
    assert!(matches!(err, McpTestError::Rpc { .. }), "{err:?}");
    let err = mcp
        .call_tool::<Sum>("sum", json!({ "a": "two" }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(-32602), "{err:?}");

    // A guard refusal (no `x-test-key` on a loopback request).
    let err = mcp.call_tool_text("locked", ()).await.unwrap_err();
    assert!(err.code().is_some(), "{err:?}");
}

// ── Resources and prompts ──────────────────────────────────────────────────

#[r2e_core::test]
async fn resources_and_prompts_round_trip() {
    let app = plain_app().await;
    let mcp = app.mcp().await;

    let resources = mcp.list_resources().await.unwrap();
    assert!(resources.iter().any(|r| r.uri == "r2e://fixture/log"));
    assert!(mcp.list_resource_templates().await.unwrap().is_empty());

    mcp.call_tool_raw("div", json!({ "a": 6.0, "b": 3.0 }))
        .await
        .unwrap();
    let log = mcp.read_resource_text("r2e://fixture/log").await.unwrap();
    assert!(log.contains("tool:divide"), "{log}");
    assert_eq!(
        mcp.read_resource("r2e://fixture/log").await.unwrap().len(),
        1
    );

    let err = mcp.read_resource("r2e://fixture/fail").await.unwrap_err();
    assert!(matches!(err, McpTestError::Rpc { .. }), "{err:?}");
    assert!(err.to_string().contains("resource exploded"), "{err}");

    let prompts = mcp.list_prompts().await.unwrap();
    assert!(prompts.iter().any(|p| p.name == "explain_div"));
    let prompt = mcp
        .get_prompt("explain_div", json!({ "a": 6.0, "b": 3.0 }))
        .await
        .unwrap();
    assert_eq!(prompt.messages.len(), 1);
    let usage = mcp.get_prompt("usage", ()).await.unwrap();
    assert_eq!(usage.messages.len(), 1);
    assert!(mcp.get_prompt("nope", ()).await.is_err());
}

// ── Notifications ──────────────────────────────────────────────────────────

#[r2e_core::test]
async fn list_changed_notifications_are_captured() {
    let app = plain_app().await;
    let mcp = app.mcp().await;
    // Notifications only flow once the session has listed something.
    mcp.tool_names().await;
    assert!(mcp.notifications().is_empty());

    app.bean::<McpSessions>().notify_list_changed();

    mcp.wait_for(|n| *n == McpNotification::ToolListChanged)
        .await;
    mcp.wait_for(|n| *n == McpNotification::ResourceListChanged)
        .await;
    mcp.wait_for(|n| *n == McpNotification::PromptListChanged)
        .await;

    mcp.clear_notifications();
    assert!(mcp.notifications().is_empty());
    // Nothing more is coming: the bounded wait is the negative assertion.
    assert_eq!(
        mcp.wait_for_within(Duration::from_millis(50), |_| true)
            .await,
        None
    );
}

#[r2e_core::test]
async fn progress_reports_are_captured() {
    let app = plain_app().await;
    let mcp = app.mcp().await;

    // The client sends a progress token with every request.
    assert_eq!(
        mcp.call_tool_text("crunch", ()).await.unwrap(),
        "requested=true"
    );
    let done = mcp
        .wait_for(|n| matches!(n, McpNotification::Progress { progress, .. } if *progress == 3.0))
        .await;
    assert_eq!(
        done,
        McpNotification::Progress {
            progress: 3.0,
            total: Some(3.0),
            message: Some("done".to_string()),
        }
    );
    // Strictly increasing: the two non-increasing reports were dropped.
    // (Handlers run concurrently: arrival order is not send order.)
    let mut steps: Vec<f64> = mcp
        .notifications()
        .into_iter()
        .filter_map(|n| match n {
            McpNotification::Progress { progress, .. } => Some(progress),
            _ => None,
        })
        .collect();
    steps.sort_by(f64::total_cmp);
    assert_eq!(steps, [1.0, 2.0, 3.0]);

    // Resource reads and prompt expansions report too.
    mcp.clear_notifications();
    mcp.read_resource("progress://report").await.unwrap();
    mcp.wait_for(|n| matches!(n, McpNotification::Progress { progress, .. } if *progress == 1.0))
        .await;
}

#[r2e_core::test]
#[allow(deprecated)] // `resources/subscribe`: still the 2025-06-18 way.
async fn resource_updates_are_captured() {
    let app = plain_app().await;
    let mcp = app.mcp().await;
    mcp.session()
        .subscribe(rmcp::model::SubscribeRequestParams::new("r2e://fixture/log"))
        .await
        .unwrap();

    app.bean::<McpResourceUpdates>().notify("r2e://fixture/log");

    let seen = mcp
        .wait_for(|n| matches!(n, McpNotification::ResourceUpdated { .. }))
        .await;
    assert_eq!(
        seen,
        McpNotification::ResourceUpdated {
            uri: "r2e://fixture/log".to_string()
        }
    );
}
