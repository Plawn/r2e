//! Pagination cursors are bound to the caller: a cursor issued to one
//! subject is refused for another, even when both see the same list.

use r2e_mcp::McpServer;
use serde_json::json;

use crate::fixtures::{
    initialize_auth, offline_auth, pinned, rpc_auth, secured_plugin_app, test_jwt,
};

#[r2e_core::test]
async fn a_cursor_replayed_by_another_subject_is_refused() {
    let router = secured_plugin_app(
        McpServer::new()
            .with_auth(offline_auth())
            .with_token_validator(pinned(&test_jwt()))
            .with_page_size(1),
    )
    .await;
    let alice = test_jwt().token_builder("alice").build();
    let bob = test_jwt().token_builder("bob").build();

    let alice_session = initialize_auth(&router, "/mcp", &alice).await;
    let first = rpc_auth(
        &router,
        "/mcp",
        &alice_session,
        &alice,
        "tools/list",
        json!({}),
    )
    .await;
    let cursor = first["result"]["nextCursor"]
        .as_str()
        .expect("paginated")
        .to_string();

    let own = rpc_auth(
        &router,
        "/mcp",
        &alice_session,
        &alice,
        "tools/list",
        json!({ "cursor": cursor }),
    )
    .await;
    assert!(own["result"]["tools"].is_array(), "{own}");

    let bob_session = initialize_auth(&router, "/mcp", &bob).await;
    let replayed = rpc_auth(
        &router,
        "/mcp",
        &bob_session,
        &bob,
        "tools/list",
        json!({ "cursor": cursor }),
    )
    .await;
    assert_eq!(replayed["error"]["code"], -32602, "{replayed}");
}
