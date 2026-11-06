//! HTTP session idle timeout (`mcp.session.idle-timeout-secs` /
//! `McpServer::with_session_idle_timeout`): rmcp's session worker quits
//! after that long without JSON-RPC traffic and the next request carrying the
//! session id is answered 404; `0` keeps the session alive indefinitely.
//!
//! The clock is virtual (`start_paused`): the worker's idle timer is a tokio
//! sleep on the same current-thread runtime, so `rt::sleep` in the test
//! advances it without waiting for real seconds.

use std::time::Duration;

use r2e_core::http::Routes;
use r2e_core::http::StatusCode;
use r2e_core::{rt, AppBuilder, R2eConfig};
use r2e_mcp::{AppBuilderMcpExt, McpServer};
use serde_json::json;

use crate::fixtures::{Calc, CallLog, FixtureTools};
use crate::support;

async fn app(yaml: &str, plugin: McpServer) -> Routes {
    AppBuilder::new()
        .override_config(R2eConfig::from_yaml_str(yaml).unwrap())
        .load_config::<()>()
        .plugin(plugin)
        .provide(Calc)
        .provide(CallLog::default())
        .build_state()
        .await
        .register_mcp_service::<FixtureTools>()
        .build()
}

async fn add(router: &Routes, session: &str) -> StatusCode {
    support::post(
        router,
        "/mcp",
        Some(session),
        &json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "add", "arguments": { "a": 1, "b": 2 } }
        }),
    )
    .await
    .status
}

/// The worker quitting and the manager forgetting the session are two steps;
/// poll briefly so the assertion sees the final state, not the gap.
async fn wait_for_gone(router: &Routes, session: &str) -> StatusCode {
    let mut status = add(router, session).await;
    for _ in 0..100 {
        if status == StatusCode::NOT_FOUND {
            break;
        }
        rt::sleep(Duration::from_millis(10)).await;
        status = add(router, session).await;
    }
    status
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn idle_session_is_closed_after_the_timeout() {
    let router = app(
        "mcp:\n  session:\n    idle-timeout-secs: 2\n",
        McpServer::new(),
    )
    .await;
    let session = support::initialize(&router, "/mcp").await;

    // Below the timeout: alive (and the call resets the timer).
    rt::sleep(Duration::from_secs(1)).await;
    assert_eq!(add(&router, &session).await, StatusCode::OK);

    // Past the timeout since the last message: gone.
    rt::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        wait_for_gone(&router, &session).await,
        StatusCode::NOT_FOUND
    );
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn zero_means_the_session_never_expires() {
    let router = app(
        "mcp:\n  session:\n    idle-timeout-secs: 0\n",
        McpServer::new(),
    )
    .await;
    let session = support::initialize(&router, "/mcp").await;

    // Well past rmcp's 300 s default.
    rt::sleep(Duration::from_secs(400)).await;
    assert_eq!(add(&router, &session).await, StatusCode::OK);
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn builder_idle_timeout_overrides_config() {
    let router = app(
        "mcp:\n  session:\n    idle-timeout-secs: 1\n",
        McpServer::new().with_session_idle_timeout(Duration::ZERO),
    )
    .await;
    let session = support::initialize(&router, "/mcp").await;

    rt::sleep(Duration::from_secs(400)).await;
    assert_eq!(add(&router, &session).await, StatusCode::OK);
}

#[r2e_core::test(flavor = "current_thread", start_paused = true)]
async fn unset_keeps_the_upstream_default() {
    let router = app("mcp:\n  path: /mcp\n", McpServer::new()).await;
    let session = support::initialize(&router, "/mcp").await;

    rt::sleep(Duration::from_secs(299)).await;
    assert_eq!(add(&router, &session).await, StatusCode::OK);
    rt::sleep(Duration::from_secs(301)).await;
    assert_eq!(
        wait_for_gone(&router, &session).await,
        StatusCode::NOT_FOUND
    );
}
