//! `r2e_mcp::testing::pin_mcp_validator` (feature `testing`): the documented
//! no-Docker fast path — pinned validator + config overrides, zero network.

use r2e_core::http::StatusCode;
use r2e_core::AppBuilder;
use r2e_mcp::testing::pin_mcp_validator;
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use r2e_test::TestJwt;

use crate::fixtures::{self, initialize_auth, RESOURCE};
use crate::support;

#[tokio::test]
async fn pinned_validator_boots_offline_and_authenticates() {
    let jwt = TestJwt::for_resource(RESOURCE);
    // Everything auth-related comes from the helper: no YAML, no `with_auth`.
    let router = pin_mcp_validator(AppBuilder::new(), &jwt, RESOURCE)
        .load_config::<()>()
        .plugin(McpServer::new())
        .build_state()
        .await
        .register_mcp_service::<fixtures::SecuredTools>()
        .build();

    // Valid token from the same TestJwt → full session handshake.
    let token = jwt.token("alice", &[]);
    let session = initialize_auth(&router, "/mcp", &token).await;
    assert!(!session.is_empty());

    // Auth is genuinely ON: a garbage token is challenged.
    let response = fixtures::post_auth(
        &router,
        "/mcp",
        None,
        &TestJwt::malformed_token(),
        &support::initialize_body(),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);

    // …and a missing token too.
    let response = support::post(&router, "/mcp", None, &support::initialize_body()).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
}
