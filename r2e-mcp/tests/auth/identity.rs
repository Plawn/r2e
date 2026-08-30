//! Task #993 — the authenticated identity is built once and shared.
//!
//! The auth layer deposits the caller twice: as the `McpPrincipal` and as the
//! identity extension `#[inject(identity)]` reads. Both point at the same
//! `Arc<AuthenticatedUser>`; a member only materializes an owned copy when it
//! actually declares an identity parameter. These are the observable
//! consequences (the allocation guard itself lives in `tests/hotpath`).

use std::sync::Arc;

use r2e_core::http::Router;
use r2e_core::prelude::*;
use r2e_core::AppBuilder;
use r2e_mcp::{AppBuilderMcpExt, McpPrincipal, McpServer, ToolCall};
use r2e_security::AuthenticatedUser;
use serde_json::json;

use crate::fixtures::{initialize_auth, offline_auth, pinned, test_jwt, tools_call_auth};

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
}

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

async fn call(router: &Router, session: &str, token: &str, name: &str) -> String {
    let result = tools_call_auth(router, "/mcp", session, token, name, json!({})).await;
    assert_eq!(result["result"]["isError"], false, "{result}");
    result["result"]["content"][0]["text"]
        .as_str()
        .expect("text content")
        .to_string()
}

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
