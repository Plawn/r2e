//! The in-process (loopback) transport — feature `loopback`. `McpLoopback`
//! opens sessions on the mounted MCP port without the network stack: the
//! same `Endpoint`, catalog and `McpSessions` as Streamable HTTP, the
//! identity resolved once at connect into the parts every message carries.
//!
//! * tools/list + tools/call round-trips; the session is a real session
//!   (`ToolCall::session_id`, `McpSessions::get`/`len`, pruned on close);
//! * identities: anonymous, trusted principal (auth off and on), bearer
//!   validated by the plugin's `McpTokenValidator` (roles, `#[tool(scopes)]`,
//!   `mcp.auth.required-scopes`, invalid / missing tokens);
//! * struct-level `#[inject(identity)]` scopes run on the synthesized parts —
//!   a header-reading identity is `Rejected` (the transport leak #1220);
//! * refusals: `SessionInUse`, `InvalidSessionId`, `NotMounted`
//!   (`mcp.enabled = false`, router dropped), `Stateless`;
//! * id lifecycle: concurrent claims admit one; a cancelled connect, a
//!   close, a drop free the id at once — even while an `McpSession` handle
//!   still holds the state;
//! * the synthesized request is `POST {mcp.path}` with the bearer's
//!   `Authorization`, visible to `#[inject(request)]` scopes;
//! * a loopback id is unknown to HTTP (404); `McpSessionInit` runs once;
//!   session-private members reach the session; app shutdown cancels it.
//!
//! The bearer tests pinning a `TestJwt` validator need `--features testing`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::Parts;
use r2e_core::http::{Routes, StatusCode, AUTHORIZATION};
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, R2eConfig, Rejection, RejectionKind};
use r2e_mcp::auth::{DiscoveryMode, McpAuthConfig, ScopePolicy};
#[cfg(feature = "testing")]
use r2e_mcp::testing::pin_mcp_validator;
use r2e_mcp::{
    AppBuilderMcpExt, DynamicTool, LoopbackError, LoopbackIdentity, LoopbackSession, McpError,
    McpLoopback, McpPrincipal, McpServer, McpSessionInit, McpSessions, McpTokenValidator,
    SessionInit, SessionToolset, ToolCall,
};
use r2e_security::{AuthenticatedUser, StandardClaims};
use r2e_test::TestJwt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::ServiceError;
use serde_json::json;

use crate::support;

// ── Fixture services ───────────────────────────────────────────────────────

/// No struct-level identity: callable anonymously, identity-aware tools
/// read whatever the connect deposited.
#[controller]
struct LoopTools;

#[routes]
impl LoopTools {
    /// Always available.
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    /// The transport session id as the tool sees it.
    #[tool]
    async fn session(&self, call: ToolCall) -> String {
        call.session_id()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "<none>".to_string())
    }

    /// The principal deposited at connect, if any.
    #[tool]
    async fn whoami(&self, #[inject(identity)] principal: Option<McpPrincipal>) -> String {
        principal
            .map(|p| p.user.sub.clone())
            .unwrap_or_else(|| "<anonymous>".to_string())
    }
}

/// The `mcp.auth`-shaped service: scope- and role-gated members.
#[controller]
struct SecuredLoopTools;

#[routes]
impl SecuredLoopTools {
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

/// An HTTP-only identity: whatever the `x-user` header says. A loopback
/// connect never carries it — the struct-level scope must reject.
#[derive(Clone)]
struct HeaderUser(String);

impl<S: Send + Sync> FromRequestParts<S> for HeaderUser {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-user")
            .and_then(|value| value.to_str().ok())
            .map(|user| HeaderUser(user.to_owned()))
            .ok_or_else(|| Rejection::new(RejectionKind::Unauthenticated, "missing x-user header"))
    }
}

#[controller]
struct HeaderTools {
    #[inject(identity)]
    user: HeaderUser,
}

#[routes]
impl HeaderTools {
    #[tool]
    async fn who(&self) -> String {
        format!("who {}", self.user.0)
    }
}

/// Struct-level principal: the connect-time identity, read by every member.
#[controller]
struct PrincipalTools {
    #[inject(identity)]
    principal: McpPrincipal,
}

#[routes]
impl PrincipalTools {
    #[tool]
    async fn who(&self) -> String {
        format!("who {}", self.principal.user.sub)
    }
}

/// What the synthesized request looks like to a `#[inject(request)]` scope:
/// method, path and the `Authorization` header (or `-`).
#[derive(Clone)]
struct RequestLine(String);

impl<S: Send + Sync> FromRequestParts<S> for RequestLine {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let authorization = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-");
        Ok(RequestLine(format!(
            "{} {} {authorization}",
            parts.method,
            parts.uri.path()
        )))
    }
}

#[controller]
struct RequestLineTools {
    #[inject(request)]
    line: RequestLine,
}

#[routes]
impl RequestLineTools {
    #[tool]
    async fn line(&self) -> String {
        self.line.0.clone()
    }
}

/// A `session_init` hook counting its runs and the ids it saw.
#[derive(Clone, Default)]
struct CountingInit {
    calls: Arc<AtomicUsize>,
    ids: Arc<Mutex<Vec<String>>>,
}

impl McpSessionInit for CountingInit {
    async fn init(&self, session: &SessionInit) -> Result<SessionToolset, McpError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(id) = session.session_id() {
            self.ids.lock().unwrap().push(id.to_string());
        }
        Ok(SessionToolset::new())
    }
}

/// A `session_init` hook that parks every session until `release` is set.
#[derive(Clone, Default)]
struct BlockingInit {
    calls: Arc<AtomicUsize>,
    release: Arc<AtomicBool>,
}

impl McpSessionInit for BlockingInit {
    async fn init(&self, _session: &SessionInit) -> Result<SessionToolset, McpError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            r2e_core::rt::sleep(Duration::from_millis(5)).await;
        }
        Ok(SessionToolset::new())
    }
}

// ── Boot helpers ───────────────────────────────────────────────────────────

/// A booted app: the router (for HTTP cross-checks) and the two beans the
/// plugin provides for this transport.
struct Booted {
    router: Routes,
    loopback: McpLoopback,
    sessions: McpSessions,
}

macro_rules! boot {
    ($builder:expr $(, $service:ty)+) => {{
        let app = $builder.build_state().await $(.register_mcp_service::<$service>())+;
        let loopback = app
            .bean_context()
            .try_get::<McpLoopback>()
            .expect("McpServer provides McpLoopback");
        let sessions = app
            .bean_context()
            .try_get::<McpSessions>()
            .expect("McpServer provides McpSessions");
        Booted {
            router: app.build(),
            loopback,
            sessions,
        }
    }};
}

async fn plain_app(plugin: McpServer) -> Booted {
    boot!(AppBuilder::new().plugin(plugin), LoopTools)
}

const RESOURCE: &str = "http://localhost:3000/mcp";
const ISSUER: &str = "http://idp.test";

/// `mcp.auth` on with a validator pinned to `jwt` (no network).
#[cfg(feature = "testing")]
async fn secured_app(jwt: &TestJwt) -> Booted {
    boot!(
        pin_mcp_validator(AppBuilder::new(), jwt, RESOURCE)
            .load_config::<()>()
            .plugin(McpServer::new()),
        SecuredLoopTools
    )
}

/// A trusted principal: a caller that already authenticated the user.
fn trusted(sub: &str, roles: &[&str], scopes: &[&str]) -> McpPrincipal {
    let claims = StandardClaims {
        sub: sub.to_string(),
        roles: Some(roles.iter().map(|r| r.to_string()).collect()),
        ..Default::default()
    };
    McpPrincipal::trusted(
        AuthenticatedUser::from_claims(claims),
        scopes.iter().map(|s| s.to_string()),
    )
}

// ── Client helpers ─────────────────────────────────────────────────────────

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text())
        .map(|t| t.text.clone())
        .collect::<Vec<_>>()
        .join("")
}

async fn call(
    session: &LoopbackSession,
    name: &'static str,
) -> Result<CallToolResult, ServiceError> {
    session.call_tool(CallToolRequestParams::new(name)).await
}

async fn call_text(session: &LoopbackSession, name: &'static str) -> String {
    let result = call(session, name).await.unwrap();
    assert_ne!(result.is_error, Some(true), "{name} failed: {result:?}");
    text(&result)
}

async fn tool_names(session: &LoopbackSession) -> Vec<String> {
    let mut names: Vec<String> = session
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    names.sort();
    names
}

/// Poll `condition` until it holds or `timeout` elapses.
async fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        r2e_core::rt::sleep(Duration::from_millis(10)).await;
    }
    condition()
}

// ── Round-trip + session identity ──────────────────────────────────────────

#[r2e_core::test]
async fn loopback_session_round_trips_and_is_a_real_session() {
    let app = plain_app(McpServer::new()).await;
    assert!(app.loopback.is_mounted());
    assert!(app.sessions.is_empty());

    let session = app
        .loopback
        .connect("agent-1", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(session.id(), "agent-1");
    assert_eq!(tool_names(&session).await, ["ping", "session", "whoami"]);
    assert_eq!(call_text(&session, "ping").await, "pong");
    // The tool sees the chosen id, the bean lists it.
    assert_eq!(call_text(&session, "session").await, "agent-1");
    assert_eq!(call_text(&session, "whoami").await, "<anonymous>");
    assert_eq!(app.sessions.len(), 1);
    let handle = app
        .sessions
        .get("agent-1")
        .expect("registered in McpSessions");
    assert_eq!(handle.id().map(|id| id.as_str()), Some("agent-1"));
    assert!(handle.subject().is_none());
    // The handle is a strong reference to the session state: release it so the
    // registry (which only holds weak ones) can forget the session.
    drop(handle);

    // Closing ends the server side: the registry no longer lists the id.
    session.close().await;
    assert!(app.sessions.get("agent-1").is_none());
    assert!(app.sessions.is_empty());
}

#[r2e_core::test]
async fn two_sessions_carry_their_own_trusted_principal() {
    // `mcp.auth` off: a trusted principal is deposited as-is and binds the
    // session to its subject, exactly as the HTTP auth layer would.
    let app = plain_app(McpServer::new()).await;
    let alice = app
        .loopback
        .connect(
            "alice-thread",
            LoopbackIdentity::Principal(trusted("alice", &["admin"], &[])),
        )
        .await
        .unwrap();
    let bob = app
        .loopback
        .connect(
            "bob-thread",
            LoopbackIdentity::Principal(trusted("bob", &[], &[])),
        )
        .await
        .unwrap();

    assert_eq!(call_text(&alice, "whoami").await, "alice");
    assert_eq!(call_text(&bob, "whoami").await, "bob");
    assert_eq!(call_text(&alice, "session").await, "alice-thread");
    assert_eq!(call_text(&bob, "session").await, "bob-thread");
    assert_eq!(app.sessions.len(), 2);
    assert_eq!(app.sessions.for_subject("alice").len(), 1);
    assert_eq!(
        app.sessions.get("bob-thread").unwrap().subject(),
        Some("bob")
    );

    alice.close().await;
    assert_eq!(app.sessions.len(), 1);
    bob.close().await;
    assert!(app.sessions.is_empty());
}

// ── Bearer / principal under `mcp.auth` ────────────────────────────────────

#[cfg(feature = "testing")]
#[r2e_core::test]
async fn bearer_is_validated_by_the_plugin_validator() {
    let jwt = TestJwt::for_resource(RESOURCE);
    let app = secured_app(&jwt).await;

    let admin = jwt
        .token_builder("alice")
        .roles(&["admin"])
        .scopes(&["mcp:read"])
        .build();
    let alice = app
        .loopback
        .connect("alice", LoopbackIdentity::Bearer(admin))
        .await
        .unwrap();
    assert_eq!(call_text(&alice, "whoami").await, "alice");
    assert_eq!(
        tool_names(&alice).await,
        ["admin_only", "read_data", "whoami"]
    );
    assert_eq!(call_text(&alice, "admin_only").await, "admin:alice");
    assert_eq!(call_text(&alice, "read_data").await, "data");
    assert_eq!(app.sessions.get("alice").unwrap().subject(), Some("alice"));

    // No role, no scope: filtered out of the list and refused on call.
    let plain = jwt.token("bob", &[]);
    let bob = app
        .loopback
        .connect("bob", LoopbackIdentity::Bearer(plain))
        .await
        .unwrap();
    assert_eq!(call_text(&bob, "whoami").await, "bob");
    assert_eq!(tool_names(&bob).await, ["whoami"]);
    assert!(call(&bob, "read_data").await.is_err());
    assert!(call(&bob, "admin_only").await.is_err());
}

#[cfg(feature = "testing")]
#[r2e_core::test]
async fn invalid_or_missing_bearer_is_unauthorized_under_auth() {
    let jwt = TestJwt::for_resource(RESOURCE);
    let app = secured_app(&jwt).await;

    let err = app
        .loopback
        .connect("bad", LoopbackIdentity::Bearer(TestJwt::malformed_token()))
        .await
        .map(drop)
        .expect_err("malformed token refused");
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");

    let err = app
        .loopback
        .connect(
            "wrong",
            LoopbackIdentity::Bearer(jwt.wrong_audience_token("eve")),
        )
        .await
        .map(drop)
        .expect_err("wrong audience refused");
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");

    // Fail-closed, like the HTTP layer: no credentials under `mcp.auth`.
    let err = app
        .loopback
        .connect("anon", LoopbackIdentity::Anonymous)
        .await
        .map(drop)
        .expect_err("anonymous refused under mcp.auth");
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");
    assert!(err.to_string().contains("missing"), "{err}");
    assert!(app.sessions.is_empty());
}

#[cfg(feature = "testing")]
#[r2e_core::test]
async fn trusted_principal_under_auth_is_treated_like_a_bearer() {
    let jwt = TestJwt::for_resource(RESOURCE);
    let app = secured_app(&jwt).await;

    let session = app
        .loopback
        .connect(
            "agent",
            LoopbackIdentity::Principal(trusted("agent-7", &["admin"], &["mcp:read"])),
        )
        .await
        .unwrap();
    assert_eq!(call_text(&session, "whoami").await, "agent-7");
    assert_eq!(
        tool_names(&session).await,
        ["admin_only", "read_data", "whoami"]
    );
    assert_eq!(call_text(&session, "admin_only").await, "admin:agent-7");

    let limited = app
        .loopback
        .connect(
            "limited",
            LoopbackIdentity::Principal(trusted("agent-8", &[], &[])),
        )
        .await
        .unwrap();
    assert_eq!(tool_names(&limited).await, ["whoami"]);
    assert!(call(&limited, "read_data").await.is_err());
}

#[r2e_core::test]
async fn required_scopes_apply_to_every_loopback_identity() {
    let jwt = TestJwt::with_config(b"loopback-secret", ISSUER, RESOURCE);
    let auth = McpAuthConfig {
        issuer: ISSUER.to_string(),
        resource: Some(RESOURCE.to_string()),
        discovery: Some(DiscoveryMode::Off),
        jwks_url: Some("http://127.0.0.1:1/jwks".to_string()),
        allow_insecure: Some(true),
        required_scopes: Some(vec!["mcp:base".to_string()]),
        ..Default::default()
    };
    let validator =
        McpTokenValidator::jwt(Arc::new(jwt.claims_validator()), ScopePolicy::default());
    let app = boot!(
        AppBuilder::new().plugin(
            McpServer::new()
                .with_auth(auth)
                .with_token_validator(validator)
        ),
        SecuredLoopTools
    );

    let without = jwt.token("alice", &[]);
    let err = app
        .loopback
        .connect("a", LoopbackIdentity::Bearer(without))
        .await
        .map(drop)
        .expect_err("bearer without the required scope refused");
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");

    let with = jwt.token_builder("alice").scopes(&["mcp:base"]).build();
    let session = app
        .loopback
        .connect("a", LoopbackIdentity::Bearer(with))
        .await
        .unwrap();
    assert_eq!(call_text(&session, "whoami").await, "alice");

    let err = app
        .loopback
        .connect("b", LoopbackIdentity::Principal(trusted("bob", &[], &[])))
        .await
        .map(drop)
        .expect_err("principal without the required scope refused");
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");

    let bob = app
        .loopback
        .connect(
            "b",
            LoopbackIdentity::Principal(trusted("bob", &[], &["mcp:base"])),
        )
        .await
        .unwrap();
    assert_eq!(call_text(&bob, "whoami").await, "bob");
}

#[r2e_core::test]
async fn bearer_without_auth_is_anonymous() {
    // No `mcp.auth`: there is no validator to run; the token is ignored and
    // the session is anonymous, as it would be over HTTP.
    let app = plain_app(McpServer::new()).await;
    let session = app
        .loopback
        .connect("t", LoopbackIdentity::Bearer("not-a-token".to_string()))
        .await
        .unwrap();
    assert_eq!(call_text(&session, "whoami").await, "<anonymous>");

    // A token that cannot be a header value is refused outright.
    let err = app
        .loopback
        .connect("bad", LoopbackIdentity::Bearer("not\ra-token".to_string()))
        .await
        .map(drop)
        .expect_err("control characters refused");
    assert!(matches!(err, LoopbackError::Unauthorized(_)), "{err}");
}

#[r2e_core::test]
async fn the_synthesized_request_is_a_post_on_the_mount_path() {
    let yaml = "mcp:\n  path: /agents/mcp\n";
    let app = boot!(
        AppBuilder::new()
            .override_config(R2eConfig::from_yaml_str(yaml).unwrap())
            .load_config::<()>()
            .plugin(McpServer::new()),
        RequestLineTools
    );

    let anonymous = app
        .loopback
        .connect("anon", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&anonymous, "line").await, "POST /agents/mcp -");

    // The bearer's header travels even without `mcp.auth`: a scope reading
    // `Authorization` sees what the HTTP request would have carried.
    let bearer = app
        .loopback
        .connect("bearer", LoopbackIdentity::Bearer("tok-1".to_string()))
        .await
        .unwrap();
    assert_eq!(
        call_text(&bearer, "line").await,
        "POST /agents/mcp Bearer tok-1"
    );

    // Same thing over the wire.
    let http = support::initialize(&app.router, "/agents/mcp").await;
    let result = support::tools_call_with_headers(
        &app.router,
        "/agents/mcp",
        &http,
        &[("authorization", "Bearer tok-2")],
        "line",
        json!({}),
    )
    .await;
    assert_eq!(
        result["result"]["content"][0]["text"],
        "POST /agents/mcp Bearer tok-2"
    );
}

// ── Struct-level request scopes ────────────────────────────────────────────

#[r2e_core::test]
async fn struct_level_principal_runs_on_the_synthesized_parts() {
    let app = boot!(AppBuilder::new().plugin(McpServer::new()), PrincipalTools);

    let session = app
        .loopback
        .connect("p", LoopbackIdentity::Principal(trusted("carol", &[], &[])))
        .await
        .unwrap();
    assert_eq!(call_text(&session, "who").await, "who carol");

    // The scope fails closed: no principal, no session.
    let err = app
        .loopback
        .connect("q", LoopbackIdentity::Anonymous)
        .await
        .map(drop)
        .expect_err("required struct-level identity refused");
    match err {
        LoopbackError::Rejected(status, body) => {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(!body.is_empty());
        }
        other => panic!("expected Rejected, got {other}"),
    }
    assert!(app.sessions.get("q").is_none());
}

#[r2e_core::test]
async fn http_only_identity_is_rejected_at_connect() {
    // An identity read from a request header exists on no loopback
    // connect: the service is unreachable from this transport (#1220).
    let app = boot!(AppBuilder::new().plugin(McpServer::new()), HeaderTools);
    let err = app
        .loopback
        .connect("h", LoopbackIdentity::Principal(trusted("dave", &[], &[])))
        .await
        .map(drop)
        .expect_err("header identity cannot be satisfied in-process");
    match err {
        LoopbackError::Rejected(status, body) => {
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(body.contains("missing x-user header"), "{body}");
        }
        other => panic!("expected Rejected, got {other}"),
    }
}

// ── Refusals ───────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn a_live_id_cannot_be_claimed_twice() {
    let app = plain_app(McpServer::new()).await;
    let first = app
        .loopback
        .connect("dup", LoopbackIdentity::Anonymous)
        .await
        .unwrap();

    let err = app
        .loopback
        .connect("dup", LoopbackIdentity::Anonymous)
        .await
        .map(drop)
        .expect_err("second claim refused");
    match err {
        LoopbackError::SessionInUse(id) => assert_eq!(id, "dup"),
        other => panic!("expected SessionInUse, got {other}"),
    }
    assert_eq!(app.sessions.len(), 1);

    // Closed → the id is free again.
    first.close().await;
    let again = app
        .loopback
        .connect("dup", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&again, "session").await, "dup");

    // Dropped (no `close`) → the id is free at once, before the aborted
    // server task gets to run its teardown.
    drop(again);
    assert!(
        app.sessions.get("dup").is_none(),
        "dropped session still registered"
    );
    let _third = app
        .loopback
        .connect("dup", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
}

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_connects_on_one_id_admit_exactly_one() {
    let app = plain_app(McpServer::new()).await;
    // Start gate: every task parks on the watch before any `connect` runs,
    // so the eight claims race for the id instead of landing one after the
    // other as the tasks happen to be scheduled.
    let (go_tx, go_rx) = r2e_core::rt::sync::watch::channel(false);
    let parked = Arc::new(AtomicUsize::new(0));
    let connects: Vec<_> = (0..8)
        .map(|_| {
            let loopback = app.loopback.clone();
            let mut go = go_rx.clone();
            let parked = Arc::clone(&parked);
            r2e_core::rt::spawn(async move {
                parked.fetch_add(1, Ordering::SeqCst);
                go.wait_for(|released| *released).await.unwrap();
                loopback.connect("race", LoopbackIdentity::Anonymous).await
            })
        })
        .collect();
    assert!(
        eventually(Duration::from_secs(5), || parked.load(Ordering::SeqCst)
            == 8)
        .await
    );
    go_tx.send(true).unwrap();

    let mut opened = Vec::new();
    let mut refused = 0;
    for connect in connects {
        match connect.await.unwrap() {
            Ok(session) => opened.push(session),
            Err(LoopbackError::SessionInUse(id)) => {
                assert_eq!(id, "race");
                refused += 1;
            }
            Err(other) => panic!("unexpected refusal: {other}"),
        }
    }
    assert_eq!(opened.len(), 1);
    assert_eq!(refused, 7);
    assert_eq!(app.sessions.len(), 1);
    assert_eq!(call_text(&opened[0], "session").await, "race");
}

#[r2e_core::test]
async fn a_cancelled_connect_releases_the_id() {
    let init = BlockingInit::default();
    let app = boot!(
        AppBuilder::new()
            .provide(init.clone())
            .plugin(McpServer::new().session_init::<BlockingInit>()),
        LoopTools
    );

    // The handshake parks in the init hook; the id is claimed meanwhile.
    let loopback = app.loopback.clone();
    let connect = r2e_core::rt::spawn(async move {
        loopback
            .connect("blocked", LoopbackIdentity::Anonymous)
            .await
            .map(drop)
    });
    assert!(
        eventually(Duration::from_secs(5), || init.calls.load(Ordering::SeqCst)
            == 1)
        .await,
        "init hook never ran"
    );
    assert!(app.sessions.get("blocked").is_some());

    // The caller gives up (drops the connect future): the claim is released
    // without waiting for the hook.
    connect.abort();
    assert!(
        eventually(Duration::from_secs(5), || app
            .sessions
            .get("blocked")
            .is_none())
        .await,
        "cancelled connect kept its id"
    );
    assert_eq!(init.calls.load(Ordering::SeqCst), 1, "hook still parked");

    init.release.store(true, Ordering::SeqCst);
    let session = app
        .loopback
        .connect("blocked", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&session, "session").await, "blocked");
    assert_eq!(init.calls.load(Ordering::SeqCst), 2);
    assert_eq!(app.sessions.len(), 1);
}

#[r2e_core::test]
async fn a_retained_handle_does_not_keep_the_id() {
    let app = plain_app(McpServer::new()).await;
    let session = app
        .loopback
        .connect("held", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    let handle = app.sessions.get("held").expect("live");

    // Closed while the app holds a handle: gone from the registry, id free.
    session.close().await;
    assert!(app.sessions.get("held").is_none());
    assert!(app.sessions.is_empty());
    let again = app
        .loopback
        .connect("held", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&again, "session").await, "held");
    let handle_again = app.sessions.get("held").expect("live again");

    // Dropped while the app holds a handle: same.
    drop(again);
    assert!(app.sessions.get("held").is_none());
    assert!(app.sessions.all().is_empty());
    let third = app
        .loopback
        .connect("held", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&third, "session").await, "held");
    assert_eq!(app.sessions.len(), 1);

    // The old handles still describe their (ended) sessions, nothing more.
    assert_eq!(handle.id().map(|id| id.as_str()), Some("held"));
    assert_eq!(handle_again.id().map(|id| id.as_str()), Some("held"));
    drop((handle, handle_again));
    assert_eq!(app.sessions.len(), 1);
}

#[r2e_core::test]
async fn invalid_session_ids_are_refused_before_any_work() {
    let app = plain_app(McpServer::new()).await;
    for id in ["", "a\r\nb", "with space", "a,b", "a;b", "é"] {
        let err = app
            .loopback
            .connect(id, LoopbackIdentity::Anonymous)
            .await
            .err()
            .unwrap_or_else(|| panic!("id {id:?} accepted"));
        match err {
            LoopbackError::InvalidSessionId(rejected) => assert_eq!(rejected, id),
            other => panic!("expected InvalidSessionId for {id:?}, got {other}"),
        }
    }
    assert!(app.sessions.is_empty());
}

#[r2e_core::test]
async fn disabled_plugin_leaves_the_bean_unmounted() {
    let yaml = "mcp:\n  enabled: false\n";
    let app = boot!(
        AppBuilder::new()
            .override_config(R2eConfig::from_yaml_str(yaml).unwrap())
            .load_config::<()>()
            .plugin(McpServer::new()),
        LoopTools
    );
    assert!(!app.loopback.is_mounted());
    let err = app
        .loopback
        .connect("x", LoopbackIdentity::Anonymous)
        .await
        .map(drop)
        .expect_err("nothing to connect to");
    assert!(matches!(err, LoopbackError::NotMounted), "{err}");
}

#[r2e_core::test]
async fn the_bean_is_mounted_by_build_not_build_state() {
    let app = AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<LoopTools>();
    let loopback = app.bean_context().try_get::<McpLoopback>().unwrap();
    // Routes stage not run yet: no endpoint to connect to.
    assert!(!loopback.is_mounted());
    let err = loopback
        .connect("early", LoopbackIdentity::Anonymous)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, LoopbackError::NotMounted), "{err}");

    let _router = app.build();
    assert!(loopback.is_mounted());
    let session = loopback
        .connect("early", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&session, "ping").await, "pong");
}

#[r2e_core::test]
async fn dropping_the_router_unmounts_the_endpoint() {
    let Booted {
        router,
        loopback,
        sessions,
    } = plain_app(McpServer::new()).await;
    let session = loopback
        .connect("live", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&session, "ping").await, "pong");

    // The router owns the wiring; the bean only observes it.
    drop(router);
    assert!(!loopback.is_mounted());
    let err = loopback
        .connect("after", LoopbackIdentity::Anonymous)
        .await
        .map(drop)
        .expect_err("nothing to connect to once the router is gone");
    assert!(matches!(err, LoopbackError::NotMounted), "{err}");

    // The open session holds the endpoint itself: it outlives the router.
    assert_eq!(call_text(&session, "ping").await, "pong");
    assert_eq!(sessions.len(), 1);
    session.close().await;
    assert!(sessions.is_empty());
}

#[r2e_core::test]
async fn stateless_servers_have_no_sessions_to_open() {
    let app = plain_app(McpServer::new().stateless(true)).await;
    assert!(app.loopback.is_mounted());
    let err = app
        .loopback
        .connect("s", LoopbackIdentity::Anonymous)
        .await
        .map(drop)
        .expect_err("stateless refuses sessions");
    assert!(matches!(err, LoopbackError::Stateless), "{err}");
}

// ── Cross-transport ────────────────────────────────────────────────────────

#[r2e_core::test]
async fn a_loopback_session_is_unknown_to_http() {
    let app = plain_app(McpServer::new()).await;
    let session = app
        .loopback
        .connect("lb-1", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&session, "ping").await, "pong");

    // The id never reached the HTTP session manager: presenting it over
    // the wire is a 404, loopback sessions do not leak onto the network.
    let response = support::post(
        &app.router,
        "/mcp",
        Some("lb-1"),
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )
    .await;
    assert_eq!(
        response.status,
        StatusCode::NOT_FOUND,
        "{}",
        response.raw_body
    );

    // And the HTTP transport keeps working next to it.
    let http = support::initialize(&app.router, "/mcp").await;
    let listed = support::tool_names(&support::tools_list(&app.router, "/mcp", &http).await);
    assert!(listed.contains(&"ping".to_string()), "{listed:?}");
    assert_eq!(app.sessions.len(), 2);
}

// ── Session hooks and per-session members ──────────────────────────────────

#[r2e_core::test]
async fn session_init_runs_once_for_a_loopback_session() {
    let init = CountingInit::default();
    let app = boot!(
        AppBuilder::new()
            .provide(init.clone())
            .plugin(McpServer::new().session_init::<CountingInit>()),
        LoopTools
    );

    let session = app
        .loopback
        .connect("hooked", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(init.calls.load(Ordering::SeqCst), 1);
    assert_eq!(*init.ids.lock().unwrap(), ["hooked"]);

    assert_eq!(call_text(&session, "ping").await, "pong");
    assert_eq!(call_text(&session, "ping").await, "pong");
    assert_eq!(init.calls.load(Ordering::SeqCst), 1);
}

#[r2e_core::test]
async fn session_private_members_reach_a_loopback_session() {
    let app = plain_app(McpServer::new()).await;
    let session = app
        .loopback
        .connect("dyn", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(tool_names(&session).await, ["ping", "session", "whoami"]);

    app.sessions
        .get("dyn")
        .unwrap()
        .add_tool(
            DynamicTool::new("extra")
                .description("Session-private")
                .handler(|_call: ToolCall| async { "extra!" }),
        )
        .unwrap();
    app.sessions.notify_list_changed();

    assert_eq!(
        tool_names(&session).await,
        ["extra", "ping", "session", "whoami"]
    );
    assert_eq!(call_text(&session, "extra").await, "extra!");
}

// ── Shutdown ───────────────────────────────────────────────────────────────

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Minimal readiness probe: any HTTP answer from the listener.
fn listener_answers(addr: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect(addr) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    if stream
        .write_all(b"GET /mcp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 16];
    stream.read(&mut buf).map(|n| n > 0).unwrap_or(false)
}

#[r2e_core::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_shutdown_cancels_live_loopback_sessions() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let yaml = format!("server:\n  port: {port}\n");
    let app = AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(&yaml).unwrap())
        .load_config::<()>()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<LoopTools>();
    let loopback = app.bean_context().try_get::<McpLoopback>().unwrap();
    let sessions = app.bean_context().try_get::<McpSessions>().unwrap();
    let app = app.prepare(&addr);
    let stop = app.stop_handle();
    let server = r2e_core::rt::spawn(async move { app.run().await.map_err(|e| e.to_string()) });

    let probe = addr.clone();
    let ready = r2e_core::rt::spawn_blocking(move || {
        for _ in 0..100 {
            if listener_answers(&probe) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    })
    .await
    .unwrap();
    assert!(ready, "server did not become ready");

    let session = loopback
        .connect("agent", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&session, "ping").await, "pong");
    assert_eq!(sessions.len(), 1);

    // The app shutdown token is relayed to the MCP transport token, which
    // every loopback session's server side is a child of.
    stop.stop();
    match r2e_core::rt::timeout(Duration::from_secs(10), server).await {
        Ok(Ok(Ok(()))) => {}
        other => panic!("server did not stop cleanly: {other:?}"),
    }
    assert!(
        eventually(Duration::from_secs(5), || sessions.get("agent").is_none()).await,
        "loopback session survived shutdown"
    );
    assert!(session.list_tools(None).await.is_err());
}
