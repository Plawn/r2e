//! Per-session member lists under OAuth: a session is bound to the principal
//! that opened it, private `Dynamic*` members keep their scope/role gates,
//! and `session_init` / `McpSessions` see the authenticated identity.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::http::{Body, Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::rt;
use r2e_core::rt::sync::Notify;
use r2e_core::AppBuilder;
use r2e_mcp::{
    AppBuilderMcpExt, DynamicTool, InMemoryMcpSessionStore, McpError, McpServer, McpSession,
    McpSessionInit, McpSessionStore, McpSessionStoreFuture, McpSessions, SessionInit,
    SessionToolset, ToolCall,
};
use serde_json::json;

use crate::fixtures::{
    get, initialize_auth, offline_auth, pinned, post_auth, send, test_jwt, tool_names,
    tools_call_auth, tools_list_auth,
};
use crate::support;

// ── Services ───────────────────────────────────────────────────────────────

#[controller]
pub struct SessionTools;

#[routes]
impl SessionTools {
    #[tool]
    async fn ping(&self) -> &'static str {
        "pong"
    }

    /// Adds a scope-gated and a role-gated private tool to this session.
    #[tool]
    async fn add_private(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.add_tool(
            DynamicTool::new("private_read")
                .scopes(&["mcp:read"])
                .handler(|_call: ToolCall| async { "secret" }),
        )?;
        session.add_tool(
            DynamicTool::new("private_admin")
                .roles(&["admin"])
                .handler(|_call: ToolCall| async { "admin secret" }),
        )?;
        Ok("added")
    }
}

#[controller]
pub struct OpsTools;

#[routes(group = "ops", opt_in)]
impl OpsTools {
    #[tool]
    async fn restart(&self) -> &'static str {
        "restarted"
    }
}

#[derive(Clone)]
pub struct RoleToolsets;

impl McpSessionInit for RoleToolsets {
    async fn init(&self, session: &SessionInit) -> Result<SessionToolset, McpError> {
        Ok(SessionToolset::new().enable_if(session.has_role("ops"), "ops"))
    }
}

async fn app_with(plugin: McpServer) -> (Routes, McpSessions) {
    let app = AppBuilder::new()
        .provide(RoleToolsets)
        .plugin(
            plugin
                .with_auth(offline_auth())
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<SessionTools>()
        .register_mcp_service::<OpsTools>();
    let sessions = app
        .bean_context()
        .try_get::<McpSessions>()
        .expect("McpServer provides McpSessions");
    (app.build(), sessions)
}

fn token(sub: &str, scopes: &[&str], roles: &[&str]) -> String {
    test_jwt()
        .token_builder(sub)
        .scopes(scopes)
        .roles(roles)
        .build()
}

// ── Session ↔ principal binding ────────────────────────────────────────────

#[r2e_core::test]
async fn replayed_session_id_with_another_principal_is_refused() {
    let (router, _) = app_with(McpServer::new()).await;
    let alice = token("alice", &[], &[]);
    let bob = token("bob", &[], &[]);
    let session = initialize_auth(&router, "/mcp", &alice).await;

    let ok = tools_call_auth(&router, "/mcp", &session, &alice, "ping", json!({})).await;
    assert_eq!(ok["result"]["content"][0]["text"], "pong", "{ok}");

    // Refused at the HTTP layer with rmcp's own "unknown session" answer:
    // bob learns nothing about whether the session exists.
    let response = post_auth(
        &router,
        "/mcp",
        Some(&session),
        &bob,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND, "{}", response.raw_body);
    assert!(response.messages.is_empty(), "{}", response.raw_body);
    let unknown = post_auth(
        &router,
        "/mcp",
        Some("no-such-session"),
        &bob,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;
    assert_eq!(response.status, unknown.status);
    assert_eq!(response.raw_body, unknown.raw_body);

    // The owner is unaffected by the rejected replay.
    let still = tools_call_auth(&router, "/mcp", &session, &alice, "ping", json!({})).await;
    assert_eq!(still["result"]["content"][0]["text"], "pong", "{still}");
}

fn session_headers<'a>(bearer: &'a str, session: &'a str) -> [(&'a str, &'a str); 4] {
    [
        ("authorization", bearer),
        ("mcp-session-id", session),
        ("mcp-protocol-version", support::PROTOCOL_VERSION),
        ("accept", "text/event-stream"),
    ]
}

#[r2e_core::test]
async fn foreign_principal_cannot_open_the_sse_stream_of_a_session() {
    let (router, _) = app_with(McpServer::new()).await;
    let alice = token("alice", &[], &[]);
    let session = initialize_auth(&router, "/mcp", &alice).await;

    let bob = format!("Bearer {}", token("bob", &[], &[]));
    // Refused before a stream opens: a regression would hang, not fail.
    let (status, _, body) = r2e_core::rt::timeout(
        std::time::Duration::from_secs(5),
        get(&router, "/mcp", &session_headers(&bob, &session)),
    )
    .await
    .expect("the foreign GET opened alice's SSE stream");
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
}

#[r2e_core::test]
async fn foreign_principal_cannot_delete_a_session() {
    let (router, sessions) = app_with(McpServer::new()).await;
    let alice = token("alice", &[], &[]);
    let session = initialize_auth(&router, "/mcp", &alice).await;

    let bob = format!("Bearer {}", token("bob", &[], &[]));
    let (status, _, body) = send(
        &router,
        "DELETE",
        "/mcp",
        &session_headers(&bob, &session),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let still = tools_call_auth(&router, "/mcp", &session, &alice, "ping", json!({})).await;
    assert_eq!(still["result"]["content"][0]["text"], "pong", "{still}");

    // The owner still can.
    let owner = format!("Bearer {alice}");
    let (status, _, body) = send(
        &router,
        "DELETE",
        "/mcp",
        &session_headers(&owner, &session),
        Body::empty(),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    let gone = post_auth(
        &router,
        "/mcp",
        Some(&session),
        &alice,
        &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
    )
    .await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.raw_body);
    // rmcp drops the closed session's handler in the background.
    for _ in 0..100 {
        if sessions.for_subject("alice").is_empty() {
            break;
        }
        r2e_core::rt::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(sessions.for_subject("alice").is_empty());
}

// ── Private members keep their gates ───────────────────────────────────────

#[r2e_core::test]
async fn private_members_are_filtered_and_enforced_by_scopes_and_roles() {
    let (router, _) = app_with(McpServer::new()).await;
    let plain = token("alice", &[], &[]);
    let session = initialize_auth(&router, "/mcp", &plain).await;
    let added = tools_call_auth(&router, "/mcp", &session, &plain, "add_private", json!({})).await;
    assert_eq!(added["result"]["content"][0]["text"], "added", "{added}");

    let tools = tool_names(&tools_list_auth(&router, "/mcp", &session, &plain).await);
    assert!(!tools.contains(&"private_read".to_string()), "{tools:?}");
    assert!(!tools.contains(&"private_admin".to_string()), "{tools:?}");
    let denied =
        tools_call_auth(&router, "/mcp", &session, &plain, "private_read", json!({})).await;
    assert!(
        denied.get("result").is_none() || denied["result"]["isError"] == true,
        "{denied}"
    );
    let denied = tools_call_auth(
        &router,
        "/mcp",
        &session,
        &plain,
        "private_admin",
        json!({}),
    )
    .await;
    assert!(
        denied.get("result").is_none() || denied["result"]["isError"] == true,
        "{denied}"
    );

    // Same subject with the scope and role: the members appear and answer.
    let strong = token("alice", &["mcp:read"], &["admin"]);
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &session, &strong).await);
    assert!(tools.contains(&"private_read".to_string()), "{tools:?}");
    assert!(tools.contains(&"private_admin".to_string()), "{tools:?}");
    let read = tools_call_auth(
        &router,
        "/mcp",
        &session,
        &strong,
        "private_read",
        json!({}),
    )
    .await;
    assert_eq!(read["result"]["content"][0]["text"], "secret", "{read}");
    let admin = tools_call_auth(
        &router,
        "/mcp",
        &session,
        &strong,
        "private_admin",
        json!({}),
    )
    .await;
    assert_eq!(
        admin["result"]["content"][0]["text"], "admin secret",
        "{admin}"
    );
}

// ── session_init sees the principal ────────────────────────────────────────

#[r2e_core::test]
async fn session_init_shapes_the_list_from_the_principal_roles() {
    let (router, _) = app_with(McpServer::new().session_init::<RoleToolsets>()).await;

    let ops = token("olive", &[], &["ops"]);
    let session = initialize_auth(&router, "/mcp", &ops).await;
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &session, &ops).await);
    assert!(tools.contains(&"restart".to_string()), "{tools:?}");

    let dev = token("dave", &[], &[]);
    let session = initialize_auth(&router, "/mcp", &dev).await;
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &session, &dev).await);
    assert!(!tools.contains(&"restart".to_string()), "{tools:?}");
    let hidden = tools_call_auth(&router, "/mcp", &session, &dev, "restart", json!({})).await;
    assert_eq!(hidden["error"]["code"], -32601, "{hidden}");
}

// ── McpSessions by subject ─────────────────────────────────────────────────

#[r2e_core::test]
async fn sessions_bean_finds_sessions_by_subject() {
    let (router, sessions) = app_with(McpServer::new()).await;
    let alice = token("alice", &[], &[]);
    let bob = token("bob", &[], &[]);
    let a1 = initialize_auth(&router, "/mcp", &alice).await;
    let _a2 = initialize_auth(&router, "/mcp", &alice).await;
    let b = initialize_auth(&router, "/mcp", &bob).await;

    // Registered at the handshake: no request needed first.
    assert_eq!(sessions.for_subject("alice").len(), 2);
    assert_eq!(sessions.for_subject("bob").len(), 1);
    assert!(sessions.for_subject("carol").is_empty());

    // Revealing a group to bob's sessions only.
    for session in sessions.for_subject("bob") {
        assert_eq!(session.subject(), Some("bob"));
        session.enable_group("ops").unwrap();
    }
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &b, &bob).await);
    assert!(tools.contains(&"restart".to_string()), "{tools:?}");
    let tools = tool_names(&tools_list_auth(&router, "/mcp", &a1, &alice).await);
    assert!(!tools.contains(&"restart".to_string()), "{tools:?}");
}

// ── Persisted sessions keep their owner ────────────────────────────────────

/// Two instances sharing one session store, both under the same OAuth
/// config: the second restores a session the first opened.
async fn pair(store: &InMemoryMcpSessionStore) -> (Routes, Routes, McpSessions) {
    let (a, _) = app_with(McpServer::new().with_session_store(store.clone())).await;
    let (b, sessions_b) = app_with(McpServer::new().with_session_store(store.clone())).await;
    (a, b, sessions_b)
}

/// rmcp serialises the restores of one id and keeps the outcome of the
/// last one for the requests that were waiting on it; the entry is removed
/// by a spawned task (`PendingRestoreGuard`), one scheduler tick after the
/// answer went out. A request landing in that tick reads the stale
/// outcome. Let it pass before the next attempt on the same id.
async fn past_rmcp_restore_window() {
    rt::sleep(Duration::from_millis(20)).await;
}

/// A store whose `load` can be held until released: a restore "in flight".
#[derive(Clone, Default)]
struct HeldLoads {
    inner: InMemoryMcpSessionStore,
    hold: Arc<AtomicBool>,
    entered: Arc<AtomicUsize>,
    release: Arc<Notify>,
}

impl McpSessionStore for HeldLoads {
    fn load(&self, id: &str) -> McpSessionStoreFuture<'_, Option<Vec<u8>>> {
        let id = id.to_owned();
        Box::pin(async move {
            if self.hold.load(Ordering::SeqCst) {
                self.entered.fetch_add(1, Ordering::SeqCst);
                self.release.notified().await;
            }
            self.inner.load(&id).await
        })
    }

    fn store(&self, id: &str, record: &[u8], ttl: Duration) -> McpSessionStoreFuture<'_, ()> {
        self.inner.store(id, record, ttl)
    }

    fn delete(&self, id: &str) -> McpSessionStoreFuture<'_, ()> {
        self.inner.delete(id)
    }
}

#[r2e_core::test]
async fn restored_session_keeps_its_owner() {
    let store = InMemoryMcpSessionStore::new();
    let (a, b, sessions_b) = pair(&store).await;
    let alice = token("alice", &[], &[]);
    let bob = token("bob", &[], &[]);
    let session = initialize_auth(&a, "/mcp", &alice).await;

    // bob presents alice's id to an instance that has never seen it: the
    // record is not restored for him, and the answer is rmcp's own
    // "unknown session" — byte for byte.
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    let refused = post_auth(&b, "/mcp", Some(&session), &bob, &list).await;
    let unknown = post_auth(&b, "/mcp", Some("no-such-session"), &bob, &list).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.raw_body);
    assert_eq!(refused.status, unknown.status);
    assert_eq!(refused.raw_body, unknown.raw_body);
    assert!(sessions_b.get(&session).is_none());

    // The owner restores it …
    past_rmcp_restore_window().await;
    let ok = tools_call_auth(&b, "/mcp", &session, &alice, "ping", json!({})).await;
    assert_eq!(ok["result"]["content"][0]["text"], "pong", "{ok}");
    assert_eq!(
        sessions_b.get(&session).and_then(|s| s.subject().map(str::to_owned)),
        Some("alice".to_owned())
    );
    // … and once live on B, the binding keeps bob out as before.
    let refused = post_auth(&b, "/mcp", Some(&session), &bob, &list).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.raw_body);
}

#[r2e_core::test]
async fn foreign_delete_leaves_the_record_intact() {
    let store = InMemoryMcpSessionStore::new();
    let (a, b, _) = pair(&store).await;
    let alice = token("alice", &[], &[]);
    let session = initialize_auth(&a, "/mcp", &alice).await;

    // bob's DELETE reaches an instance where the session is not live: no
    // binding to refuse it, and rmcp accepts a DELETE for any id — the
    // same answer as for an id nobody ever issued — but the record stays,
    // because it is not his.
    let bob = format!("Bearer {}", token("bob", &[], &[]));
    let (status, _, body) =
        send(&b, "DELETE", "/mcp", &session_headers(&bob, &session), Body::empty()).await;
    let (unknown_status, _, unknown_body) = send(
        &b,
        "DELETE",
        "/mcp",
        &session_headers(&bob, "no-such-session"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!((status, body), (unknown_status, unknown_body));
    assert!(store.contains(&session));
    let ok = tools_call_auth(&b, "/mcp", &session, &alice, "ping", json!({})).await;
    assert_eq!(ok["result"]["content"][0]["text"], "pong", "{ok}");

    // The owner's DELETE removes the record: no instance restores it any
    // more. A still serves its own live copy — a DELETE on one instance
    // does not evict the others (documented limit).
    let owner = format!("Bearer {alice}");
    let (status, _, body) =
        send(&b, "DELETE", "/mcp", &session_headers(&owner, &session), Body::empty()).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(!store.contains(&session));
    let list = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" });
    let gone = post_auth(&b, "/mcp", Some(&session), &alice, &list).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{}", gone.raw_body);
    let local = post_auth(&a, "/mcp", Some(&session), &alice, &list).await;
    assert_eq!(local.status, StatusCode::OK, "{}", local.raw_body);
}

/// alice's restore is in flight on B when bob presents the same id: rmcp
/// makes him wait for her restore, then serves him on it — and the layer
/// withholds that answer, because the session is now bound to alice.
#[r2e_core::test]
async fn a_foreign_waiter_on_a_concurrent_restore_is_refused() {
    let store = HeldLoads::default();
    let (a, _) = app_with(McpServer::new().with_session_store(store.clone())).await;
    let (b, sessions_b) = app_with(McpServer::new().with_session_store(store.clone())).await;
    let alice = token("alice", &[], &[]);
    let bob = token("bob", &[], &[]);
    let session = initialize_auth(&a, "/mcp", &alice).await;
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });

    store.hold.store(true, Ordering::SeqCst);
    let owner = {
        let (b, session, alice) = (b.clone(), session.clone(), alice.clone());
        rt::spawn(async move { tools_call_auth(&b, "/mcp", &session, &alice, "ping", json!({})).await })
    };
    for _ in 0..100 {
        if store.entered.load(Ordering::SeqCst) == 1 {
            break;
        }
        rt::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(store.entered.load(Ordering::SeqCst), 1);
    let waiter = {
        let (b, session, bob, list) = (b.clone(), session.clone(), bob.clone(), list.clone());
        rt::spawn(async move { post_auth(&b, "/mcp", Some(&session), &bob, &list).await })
    };
    // Let bob reach rmcp's wait on the pending restore, then let it finish.
    rt::sleep(Duration::from_millis(20)).await;
    store.hold.store(false, Ordering::SeqCst);
    store.release.notify_one();

    let ok = owner.await.unwrap();
    assert_eq!(ok["result"]["content"][0]["text"], "pong", "{ok}");
    let refused = waiter.await.unwrap();
    let unknown = post_auth(&b, "/mcp", Some("no-such-session"), &bob, &list).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.raw_body);
    assert_eq!(refused.raw_body, unknown.raw_body);
    assert_eq!(
        sessions_b.get(&session).and_then(|s| s.subject().map(str::to_owned)),
        Some("alice".to_owned())
    );
}

#[r2e_core::test]
async fn anonymous_and_authenticated_records_do_not_cross() {
    let store = InMemoryMcpSessionStore::new();
    // An instance without auth and one with, on the same store.
    let open = AppBuilder::new()
        .plugin(McpServer::new().with_session_store(store.clone()))
        .build_state()
        .await
        .register_mcp_service::<SessionTools>()
        .build();
    let (secured, _) = app_with(McpServer::new().with_session_store(store.clone())).await;
    let alice = token("alice", &[], &[]);
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });

    // A record written with no subject is not restorable by an
    // authenticated caller — but it is by another open instance …
    let anonymous = support::initialize(&open, "/mcp").await;
    let refused = post_auth(&secured, "/mcp", Some(&anonymous), &alice, &list).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.raw_body);
    let open_2 = AppBuilder::new()
        .plugin(McpServer::new().with_session_store(store.clone()))
        .build_state()
        .await
        .register_mcp_service::<SessionTools>()
        .build();
    let restored = support::post(&open_2, "/mcp", Some(&anonymous), &list).await;
    assert_eq!(restored.status, StatusCode::OK, "{}", restored.raw_body);

    // … and alice's record is not restorable where there is no caller,
    // while another secured instance restores it for her.
    let owned = initialize_auth(&secured, "/mcp", &alice).await;
    let refused = support::post(&open, "/mcp", Some(&owned), &list).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND, "{}", refused.raw_body);
    let (secured_2, _) = app_with(McpServer::new().with_session_store(store.clone())).await;
    let restored = post_auth(&secured_2, "/mcp", Some(&owned), &alice, &list).await;
    assert_eq!(restored.status, StatusCode::OK, "{}", restored.raw_body);
}
