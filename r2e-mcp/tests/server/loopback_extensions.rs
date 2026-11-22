//! Typed request extensions on a loopback session (#1445) — feature
//! `loopback`. `McpLoopback::session(id).extension(T).connect()` deposits `T`
//! in the synthesized request parts, where an upstream HTTP layer would have
//! put it:
//!
//! * read by `ToolCall::extension`, `SessionInit::extension`, a
//!   `#[inject(request)]` field and a guard, on every message of the session;
//! * `ExtensionTenantResolver` resolves the tenant from it — no `Tenancy`
//!   layer runs on this transport;
//! * the session identity is not an extension: a smuggled `McpPrincipal` is
//!   dropped;
//! * nothing changes over HTTP: a wire client carries no such extension.

use std::sync::{Arc, Mutex};

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::header::Parts;
use r2e_core::http::{Routes, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, Guard, GuardContext, Identity, Rejection, RejectionKind};
use r2e_mcp::{
    AppBuilderMcpExt, LoopbackError, LoopbackIdentity, LoopbackSession, McpError, McpLoopback,
    McpPrincipal, McpServer, McpSessionInit, SessionInit, SessionToolset, ToolCall,
};
use r2e_security::{AuthenticatedUser, StandardClaims};
use r2e_tenant::{ExtensionTenantResolver, Tenancy, TenantId};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::ServiceError;

use crate::support;

// ── Fixtures ───────────────────────────────────────────────────────────────

/// What the in-process caller knows and no header carries.
#[derive(Clone, Debug, PartialEq)]
struct Workspace(String);

/// A second, unrelated extension type.
#[derive(Clone, Debug, PartialEq)]
struct Locale(&'static str);

/// Lets the call through only when the session carries a `Workspace`.
struct WorkspaceGuard;

impl SelfBuilt for WorkspaceGuard {}

impl<I: Identity> Guard<I> for WorkspaceGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Rejection>> + Send {
        let present = ctx.extensions.get::<Workspace>().is_some();
        async move {
            if present {
                Ok(())
            } else {
                Err(HttpError::forbidden("no workspace").into())
            }
        }
    }
}

#[controller]
struct ExtensionTools;

#[routes]
impl ExtensionTools {
    /// The extensions as a member reads them by hand.
    #[tool]
    async fn seen(&self, call: ToolCall) -> String {
        let workspace = call
            .extension::<Workspace>()
            .map_or_else(|| "-".to_string(), |w| w.0);
        let locale = call.extension::<Locale>().map_or("-", |l| l.0);
        format!("{workspace}/{locale}")
    }

    /// Guarded on the extension.
    #[tool]
    #[guard(WorkspaceGuard)]
    async fn guarded(&self) -> &'static str {
        "let through"
    }

    /// The principal, if any.
    #[tool]
    async fn whoami(&self, #[inject(identity)] principal: Option<McpPrincipal>) -> String {
        principal.map_or_else(|| "<anonymous>".to_string(), |p| p.user.sub.clone())
    }
}

/// The extension as a request-scoped value: absent, the scope rejects.
impl<S: Send + Sync> FromRequestParts<S> for Workspace {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Workspace>()
            .cloned()
            .ok_or_else(|| Rejection::new(RejectionKind::Unauthenticated, "no workspace"))
    }
}

#[controller]
struct ScopedTools {
    #[inject(request)]
    workspace: Workspace,
}

#[routes]
impl ScopedTools {
    #[tool]
    async fn scoped(&self) -> String {
        self.workspace.0.clone()
    }
}

/// The tenant, resolved by the `Tenancy` router from the request parts.
#[controller]
struct TenantTools {
    #[inject(request)]
    tenant: TenantId,
}

#[routes]
impl TenantTools {
    #[tool]
    async fn tenant(&self) -> String {
        self.tenant.as_str().to_owned()
    }
}

type WorkspaceResolver = ExtensionTenantResolver<Workspace, fn(&Workspace) -> Option<TenantId>>;

fn workspace_tenant(workspace: &Workspace) -> Option<TenantId> {
    TenantId::parse(&workspace.0).ok()
}

/// Records the extension each session opened with.
#[derive(Clone, Default)]
struct RecordingInit(Arc<Mutex<Vec<Option<Workspace>>>>);

impl McpSessionInit for RecordingInit {
    async fn init(&self, session: &SessionInit) -> Result<SessionToolset, McpError> {
        self.0.lock().unwrap().push(session.extension::<Workspace>());
        Ok(SessionToolset::new())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

struct Booted {
    router: Routes,
    loopback: McpLoopback,
}

macro_rules! boot {
    ($builder:expr $(, $service:ty)+) => {{
        let app = $builder.build_state().await $(.register_mcp_service::<$service>())+;
        let loopback = app
            .bean_context()
            .try_get::<McpLoopback>()
            .expect("McpServer provides McpLoopback");
        Booted {
            router: app.build(),
            loopback,
        }
    }};
}

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

/// Whether the call was refused — as a JSON-RPC error or an `isError` result.
async fn refused(session: &LoopbackSession, name: &'static str) -> bool {
    match call(session, name).await {
        Ok(result) => result.is_error == Some(true),
        Err(_) => true,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn extensions_reach_every_call_of_the_session() {
    let app = boot!(AppBuilder::new().plugin(McpServer::new()), ExtensionTools);
    let session = app
        .loopback
        .session("ext-1")
        .extension(Workspace("acme".into()))
        .extension(Locale("fr"))
        .connect()
        .await
        .unwrap();
    assert_eq!(session.id(), "ext-1");

    // Session-scoped: not just the handshake, every message.
    assert_eq!(call_text(&session, "seen").await, "acme/fr");
    assert_eq!(call_text(&session, "seen").await, "acme/fr");
    assert_eq!(call_text(&session, "guarded").await, "let through");

    // A session without them sees none, and the guard refuses.
    let bare = app.loopback.session("ext-2").connect().await.unwrap();
    assert_eq!(call_text(&bare, "seen").await, "-/-");
    assert!(refused(&bare, "guarded").await);

    // `connect(id, identity)` is unchanged: no extension.
    let classic = app
        .loopback
        .connect("ext-3", LoopbackIdentity::Anonymous)
        .await
        .unwrap();
    assert_eq!(call_text(&classic, "seen").await, "-/-");
}

#[r2e_core::test]
async fn one_value_per_type_the_last_wins() {
    let app = boot!(AppBuilder::new().plugin(McpServer::new()), ExtensionTools);
    let session = app
        .loopback
        .session("ext-twice")
        .extension(Workspace("first".into()))
        .extension(Workspace("second".into()))
        .connect()
        .await
        .unwrap();
    assert_eq!(call_text(&session, "seen").await, "second/-");
}

#[r2e_core::test]
async fn a_request_scoped_field_reads_the_extension() {
    let app = boot!(AppBuilder::new().plugin(McpServer::new()), ScopedTools);
    let session = app
        .loopback
        .session("scoped-1")
        .extension(Workspace("acme".into()))
        .connect()
        .await
        .unwrap();
    assert_eq!(call_text(&session, "scoped").await, "acme");

    // The scope runs at connect, on the same parts: without the extension
    // it rejects, as it would reject the HTTP request.
    let err = app
        .loopback
        .session("scoped-2")
        .connect()
        .await
        .expect_err("the request scope needs the extension");
    match err {
        LoopbackError::Rejected(status, body) => {
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[r2e_core::test]
async fn session_init_reads_the_extension() {
    let init = RecordingInit::default();
    let app = boot!(
        AppBuilder::new()
            .provide(init.clone())
            .plugin(McpServer::new().session_init::<RecordingInit>()),
        ExtensionTools
    );
    let _with = app
        .loopback
        .session("init-1")
        .extension(Workspace("acme".into()))
        .connect()
        .await
        .unwrap();
    let _without = app.loopback.session("init-2").connect().await.unwrap();
    assert_eq!(
        *init.0.lock().unwrap(),
        [Some(Workspace("acme".into())), None]
    );
}

#[r2e_core::test]
async fn the_tenant_resolver_reads_the_extension_without_the_tenancy_layer() {
    let resolver: WorkspaceResolver = ExtensionTenantResolver::new(workspace_tenant);
    let app = boot!(
        AppBuilder::new()
            .provide(resolver)
            .plugin(Tenancy::resolver::<WorkspaceResolver>())
            .plugin(McpServer::new()),
        TenantTools
    );

    let acme = app
        .loopback
        .session("tenant-acme")
        .extension(Workspace("acme".into()))
        .connect()
        .await
        .unwrap();
    let globex = app
        .loopback
        .session("tenant-globex")
        .extension(Workspace("globex".into()))
        .connect()
        .await
        .unwrap();
    assert_eq!(call_text(&acme, "tenant").await, "acme");
    assert_eq!(call_text(&globex, "tenant").await, "globex");

    // No extension, no tenant: the default `tenancy.on-missing` refuses.
    let err = app
        .loopback
        .session("tenant-none")
        .connect()
        .await
        .expect_err("no tenant to resolve");
    assert!(matches!(err, LoopbackError::Rejected(..)), "{err:?}");
}

#[r2e_core::test]
async fn the_identity_is_not_an_extension() {
    let app = boot!(AppBuilder::new().plugin(McpServer::new()), ExtensionTools);
    let smuggled = McpPrincipal::trusted(
        AuthenticatedUser::from_claims(StandardClaims {
            sub: "mallory".to_string(),
            ..Default::default()
        }),
        std::iter::empty::<String>(),
    );
    let trusted = McpPrincipal::trusted(
        AuthenticatedUser::from_claims(StandardClaims {
            sub: "alice".to_string(),
            ..Default::default()
        }),
        std::iter::empty::<String>(),
    );

    // Anonymous stays anonymous…
    let anonymous = app
        .loopback
        .session("id-1")
        .extension(smuggled.clone())
        .connect()
        .await
        .unwrap();
    assert_eq!(call_text(&anonymous, "whoami").await, "<anonymous>");

    // …and the declared identity is the one served.
    let alice = app
        .loopback
        .session("id-2")
        .extension(smuggled)
        .identity(LoopbackIdentity::Principal(trusted))
        .connect()
        .await
        .unwrap();
    assert_eq!(call_text(&alice, "whoami").await, "alice");
}

#[r2e_core::test]
async fn http_requests_carry_no_loopback_extension() {
    let app = boot!(AppBuilder::new().plugin(McpServer::new()), ExtensionTools);
    let _loop = app
        .loopback
        .session("ext-http")
        .extension(Workspace("acme".into()))
        .connect()
        .await
        .unwrap();

    let http = support::initialize(&app.router, "/mcp").await;
    let result = support::tools_call(
        &app.router,
        "/mcp",
        &http,
        "seen",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(result["result"]["content"][0]["text"], "-/-", "{result}");
}
