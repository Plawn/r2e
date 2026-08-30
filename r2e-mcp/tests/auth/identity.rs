//! Task #993 — the authenticated identity is built once and shared.
//!
//! The auth layer deposits the caller twice: as the `McpPrincipal` and as the
//! identity extension `#[inject(identity)]` reads. Both point at the same
//! `Arc<AuthenticatedUser>`; a member only materializes an owned copy when it
//! actually declares an identity parameter. These are the observable
//! consequences (the allocation guard itself lives in `tests/hotpath`).
//!
//! Three layers of coverage:
//!
//! * `*Call::identity::<T>()` in isolation — the resolution ladder (`Arc<T>`
//!   first, plain `T` as the fallback, `None`), on all three call types;
//! * the same end to end, through a real dispatch — including a layer that
//!   deposits a plain `AuthenticatedUser` (the fallback arm) and one that
//!   deposits both (the shared `Arc` must win);
//! * identity parameters on `#[resource]` and `#[prompt]` members, not just
//!   `#[tool]` — the three families share one codegen path, and what they
//!   share is `identity::<T>()`.

use std::sync::Arc;

use r2e_core::http::middleware::{from_fn, Next};
use r2e_core::http::{Body, Parts, Request, Router};
use r2e_core::prelude::*;
use r2e_core::rt::CancelToken;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpPrincipal, McpServer, PromptCall, ResourceCall, ToolCall};
use r2e_security::AuthenticatedUser;
use serde_json::{json, Value};

use crate::fixtures::{initialize_auth, offline_auth, pinned, rpc_auth, test_jwt, tools_call_auth};
use crate::support;

// ── Fixture service ────────────────────────────────────────────────────────

#[controller]
struct IdentityTools;

#[mcp_routes]
impl IdentityTools {
    /// Whether the identity extension and the principal are the SAME
    /// `AuthenticatedUser` allocation.
    ///
    /// Reported rather than asserted: a panic inside a member never reaches
    /// the caller (the session task swallows it), so the diagnosis has to
    /// travel as a value.
    #[tool]
    async fn sharing(&self, call: ToolCall) -> String {
        match (
            call.extension::<McpPrincipal>(),
            call.extension::<Arc<AuthenticatedUser>>(),
        ) {
            (Some(principal), Some(identity)) => format!(
                "shared={} sub={}",
                Arc::ptr_eq(&principal.user, &identity),
                identity.sub
            ),
            (None, _) => "no-principal".to_string(),
            (_, None) => "no-shared-identity".to_string(),
        }
    }

    /// The identity extractor still yields an owned `AuthenticatedUser`: the
    /// sharing is an internal representation change, not an API change.
    #[tool]
    async fn owned(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        format!("{}:{}", user.sub, user.roles.len())
    }

    /// A member may take the shared handle directly and skip the copy.
    #[tool]
    async fn borrowed(&self, call: ToolCall) -> String {
        match call.identity::<Arc<AuthenticatedUser>>() {
            Some(identity) => identity.sub.clone(),
            None => "no-shared-identity".to_string(),
        }
    }

    /// An optional identity: `None` when nothing authenticated the request.
    #[tool]
    async fn maybe_owned(&self, #[inject(identity)] user: Option<AuthenticatedUser>) -> String {
        user.map_or_else(|| "anonymous".to_string(), |u| u.sub)
    }

    // ── the same, on the other two member families ─────────────────────────

    /// A resource reading a REQUIRED identity.
    #[resource(uri = "r2e://identity/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        format!("{}:{}", user.sub, user.roles.len())
    }

    /// A resource reading an OPTIONAL identity.
    #[resource(uri = "r2e://identity/maybe")]
    async fn maybe_me(&self, #[inject(identity)] user: Option<AuthenticatedUser>) -> String {
        user.map_or_else(|| "anonymous".to_string(), |u| u.sub)
    }

    /// A resource behind the shared `#[roles]` guard, reading the identity
    /// the guard gates on.
    #[resource(uri = "r2e://identity/admin")]
    #[roles("admin")]
    async fn admin_resource(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        format!("admin-resource:{}", user.sub)
    }

    /// A prompt reading a REQUIRED identity.
    #[prompt(name = "greet")]
    async fn greet(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        format!("Hello {}.", user.sub)
    }

    /// A prompt reading an OPTIONAL identity.
    #[prompt(name = "maybe_greet")]
    async fn maybe_greet(&self, #[inject(identity)] user: Option<AuthenticatedUser>) -> String {
        format!(
            "Hello {}.",
            user.map_or_else(|| "anonymous".to_string(), |u| u.sub)
        )
    }

    /// A prompt behind the shared `#[roles]` guard.
    #[prompt(name = "admin_brief")]
    #[roles("admin")]
    async fn admin_brief(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        format!("admin-prompt:{}", user.sub)
    }
}

// ── Boot helpers ───────────────────────────────────────────────────────────

/// The authenticated app: the MCP auth layer deposits `Arc<AuthenticatedUser>`.
async fn app() -> Router {
    AppBuilder::new()
        .plugin(
            McpServer::new()
                .with_auth(offline_auth())
                .with_token_validator(pinned(&test_jwt())),
        )
        .build_state()
        .await
        .register_mcp_service::<IdentityTools>()
        .build()
}

/// The same service with NO MCP auth — nothing deposits an identity.
async fn open_app() -> Router {
    AppBuilder::new()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<IdentityTools>()
        .build()
}

/// An owned `AuthenticatedUser` for `sub`, minted through the real validator
/// (the type is not literal-constructible).
async fn user(sub: &str) -> AuthenticatedUser {
    let jwt = test_jwt();
    let token = jwt.token_builder(sub).roles(&["admin"]).build();
    let principal = pinned(&jwt)
        .validate(&token)
        .await
        .expect("fixture token validates");
    (*principal.user).clone()
}

/// A router layer depositing `plain` as a bare `AuthenticatedUser` extension —
/// what any identity layer that is not MCP's own would do.
fn depositing_plain(router: Router, plain: AuthenticatedUser) -> Router {
    router.layer(from_fn(move |mut req: Request, next: Next| {
        let plain = plain.clone();
        async move {
            req.extensions_mut().insert(plain);
            next.run(req).await
        }
    }))
}

async fn call(router: &Router, session: &str, token: &str, name: &str) -> String {
    let result = tools_call_auth(router, "/mcp", session, token, name, json!({})).await;
    assert_eq!(result["result"]["isError"], false, "{result}");
    result["result"]["content"][0]["text"]
        .as_str()
        .expect("text content")
        .to_string()
}

// ── `identity::<T>()` in isolation ─────────────────────────────────────────

/// Uniform access to the three inherent `identity` methods, so the ladder can
/// be asserted identically on all of them.
trait IdentityCall {
    fn resolve<T: Clone + Send + Sync + 'static>(&self) -> Option<T>;
}

macro_rules! impl_identity_call {
    ($($ty:ty),*) => {$(
        impl IdentityCall for $ty {
            fn resolve<T: Clone + Send + Sync + 'static>(&self) -> Option<T> {
                self.identity::<T>()
            }
        }
    )*};
}
impl_identity_call!(ToolCall, ResourceCall, PromptCall);

/// Request parts carrying the given extensions.
fn parts_with(
    shared: Option<Arc<AuthenticatedUser>>,
    plain: Option<AuthenticatedUser>,
) -> Arc<Parts> {
    let (mut parts, _) = Request::builder()
        .uri("/mcp")
        .body(Body::empty())
        .unwrap()
        .into_parts();
    if let Some(shared) = shared {
        parts.extensions.insert(shared);
    }
    if let Some(plain) = plain {
        parts.extensions.insert(plain);
    }
    Arc::new(parts)
}

fn tool_call(parts: Option<Arc<Parts>>) -> ToolCall {
    ToolCall {
        arguments: json!({}),
        parts,
        request_id: "1".to_string(),
        cancel: CancelToken::new(),
    }
}

fn resource_call(parts: Option<Arc<Parts>>) -> ResourceCall {
    ResourceCall {
        uri: "r2e://identity/me".to_string(),
        variables: Default::default(),
        parts,
        request_id: "1".to_string(),
        cancel: CancelToken::new(),
    }
}

fn prompt_call(parts: Option<Arc<Parts>>) -> PromptCall {
    PromptCall {
        arguments: json!({}),
        parts,
        request_id: "1".to_string(),
        cancel: CancelToken::new(),
    }
}

/// Walk the whole ladder for one call type.
fn assert_ladder<C: IdentityCall>(
    family: &str,
    make: impl Fn(Option<Arc<Parts>>) -> C,
    shared: &Arc<AuthenticatedUser>,
    plain: &AuthenticatedUser,
) {
    let sub = |call: &C| call.resolve::<AuthenticatedUser>().map(|u| u.sub);

    // Both present: the shared handle wins over a plain extension naming
    // somebody else.
    let both = make(Some(parts_with(
        Some(Arc::clone(shared)),
        Some(plain.clone()),
    )));
    assert_eq!(
        sub(&both).as_deref(),
        Some("alice"),
        "{family}: a plain-T extension shadowed the shared Arc<T>",
    );

    // Only the plain one: the fallback arm.
    assert_eq!(
        sub(&make(Some(parts_with(None, Some(plain.clone()))))).as_deref(),
        Some("impostor"),
        "{family}: the plain-T fallback did not resolve",
    );

    // Only the shared one: the primary arm.
    assert_eq!(
        sub(&make(Some(parts_with(Some(Arc::clone(shared)), None)))).as_deref(),
        Some("alice"),
        "{family}: the shared Arc<T> did not resolve",
    );

    // Neither extension, and no parts at all.
    assert_eq!(sub(&make(Some(parts_with(None, None)))), None, "{family}");
    assert_eq!(sub(&make(None)), None, "{family}: no parts must be None");

    // Asking for the handle itself hits the fallback arm and returns the very
    // `Arc` that was deposited — no copy at all.
    let handle = both
        .resolve::<Arc<AuthenticatedUser>>()
        .expect("shared handle");
    assert!(
        Arc::ptr_eq(&handle, shared),
        "{family}: identity::<Arc<T>>() returned a copy, not the deposited handle",
    );
}

#[tokio::test]
async fn identity_prefers_the_shared_arc_and_falls_back_to_a_plain_extension() {
    let shared = Arc::new(user("alice").await);
    let plain = user("impostor").await;

    assert_ladder("ToolCall", tool_call, &shared, &plain);
    assert_ladder("ResourceCall", resource_call, &shared, &plain);
    assert_ladder("PromptCall", prompt_call, &shared, &plain);
}

// ── End to end ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_principal_and_the_identity_extension_share_one_user() {
    let jwt = test_jwt();
    let token = jwt.token_builder("alice").roles(&["admin"]).build();
    let router = app().await;
    let session = initialize_auth(&router, "/mcp", &token).await;

    assert_eq!(
        call(&router, &session, &token, "sharing").await,
        "shared=true sub=alice",
        "the identity extension is a second copy of the principal's user",
    );
    assert_eq!(call(&router, &session, &token, "owned").await, "alice:1");
    assert_eq!(call(&router, &session, &token, "borrowed").await, "alice");
}

/// A layer depositing a plain `AuthenticatedUser` (no `Arc`) still feeds
/// `#[inject(identity)]`, through the fallback arm.
#[tokio::test]
async fn a_plain_identity_extension_still_reaches_an_identity_parameter() {
    let router = depositing_plain(open_app().await, user("layer-user").await);
    let session = support::initialize(&router, "/mcp").await;

    let message = support::tools_call(&router, "/mcp", &session, "owned", json!({})).await;
    assert_eq!(
        message["result"]["content"][0]["text"], "layer-user:1",
        "the plain-T fallback did not reach the identity parameter: {message}",
    );
}

/// With BOTH extensions present and naming different callers, the validated
/// principal wins: a plain extension can never shadow it.
#[tokio::test]
async fn the_shared_arc_wins_over_a_plain_extension_carrying_another_caller() {
    let jwt = test_jwt();
    let token = jwt.token_builder("alice").roles(&["admin"]).build();
    let router = depositing_plain(app().await, user("impostor").await);
    let session = initialize_auth(&router, "/mcp", &token).await;

    assert_eq!(
        call(&router, &session, &token, "owned").await,
        "alice:1",
        "a plain AuthenticatedUser extension shadowed the validated principal",
    );
}

/// Nothing deposits an identity: a required one is a JSON-RPC `unauthorized`
/// error, an optional one is `None`.
#[tokio::test]
async fn an_unauthenticated_request_has_no_identity() {
    let router = open_app().await;
    let session = support::initialize(&router, "/mcp").await;

    let message = support::tools_call(&router, "/mcp", &session, "maybe_owned", json!({})).await;
    assert_eq!(
        message["result"]["content"][0]["text"], "anonymous",
        "{message}"
    );

    let message = support::tools_call(&router, "/mcp", &session, "owned", json!({})).await;
    assert_eq!(message["error"]["data"], "unauthorized", "{message}");
}

// ── Resource and prompt identity ───────────────────────────────────────────

fn resource_text(message: &Value) -> &str {
    message["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no resource text in {message}"))
}

fn prompt_text(message: &Value) -> &str {
    message["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no prompt text in {message}"))
}

async fn read(router: &Router, session: &str, token: &str, uri: &str) -> Value {
    rpc_auth(
        router,
        "/mcp",
        session,
        token,
        "resources/read",
        json!({ "uri": uri }),
    )
    .await
}

async fn get(router: &Router, session: &str, token: &str, name: &str) -> Value {
    rpc_auth(
        router,
        "/mcp",
        session,
        token,
        "prompts/get",
        json!({ "name": name, "arguments": {} }),
    )
    .await
}

/// `#[inject(identity)]` on `#[resource]` and `#[prompt]` members: the same
/// `identity::<T>()` resolution the tool path uses, `Option<T>` included, plus
/// a shared `#[roles]` guard reading the caller it gates on.
#[tokio::test]
async fn resource_and_prompt_members_receive_the_identity() {
    let jwt = test_jwt();
    let token = jwt.token_builder("alice").roles(&["admin"]).build();
    let router = app().await;
    let session = initialize_auth(&router, "/mcp", &token).await;

    assert_eq!(
        resource_text(&read(&router, &session, &token, "r2e://identity/me").await),
        "alice:1"
    );
    assert_eq!(
        resource_text(&read(&router, &session, &token, "r2e://identity/maybe").await),
        "alice"
    );
    assert_eq!(
        resource_text(&read(&router, &session, &token, "r2e://identity/admin").await),
        "admin-resource:alice"
    );

    assert_eq!(
        prompt_text(&get(&router, &session, &token, "greet").await),
        "Hello alice."
    );
    assert_eq!(
        prompt_text(&get(&router, &session, &token, "maybe_greet").await),
        "Hello alice."
    );
    assert_eq!(
        prompt_text(&get(&router, &session, &token, "admin_brief").await),
        "admin-prompt:alice"
    );
}

/// The `#[roles]` guard on a resource/prompt denies a caller without the role,
/// while the un-gated members still see that caller.
#[tokio::test]
async fn role_guarded_resources_and_prompts_deny_a_caller_without_the_role() {
    let jwt = test_jwt();
    let token = jwt.token_builder("bob").roles(&["user"]).build();
    let router = app().await;
    let session = initialize_auth(&router, "/mcp", &token).await;

    assert_eq!(
        resource_text(&read(&router, &session, &token, "r2e://identity/me").await),
        "bob:1"
    );

    let denied = read(&router, &session, &token, "r2e://identity/admin").await;
    assert_eq!(denied["error"]["code"], -32600, "{denied}");
    assert_eq!(denied["error"]["data"], "forbidden", "{denied}");

    let denied = get(&router, &session, &token, "admin_brief").await;
    assert_eq!(denied["error"]["code"], -32600, "{denied}");
    assert_eq!(denied["error"]["data"], "forbidden", "{denied}");
}

/// Optional identity on a resource/prompt with nothing authenticated: `None`,
/// not a denial.
#[tokio::test]
async fn optional_identity_on_resources_and_prompts_is_none_when_unauthenticated() {
    let router = open_app().await;
    let session = support::initialize(&router, "/mcp").await;

    let message = support::resources_read(&router, "/mcp", &session, "r2e://identity/maybe").await;
    assert_eq!(resource_text(&message), "anonymous");

    let message = support::prompts_get(&router, "/mcp", &session, "maybe_greet", json!({})).await;
    assert_eq!(prompt_text(&message), "Hello anonymous.");
}
