//! Bridge between MCP tool dispatch and R2E's shared guard machinery.
//!
//! MCP reuses [`Guard<I>`](r2e_core::Guard) / [`GuardContext`] directly — the
//! same `#[roles]`, `#[all_roles]`, `#[guard]` specs (and every user
//! `#[derive(DecoratorBean)]` guard) work on tools with zero new impls. The
//! streamable-HTTP transport hands each call its originating HTTP request
//! parts, so a tool guard sees real headers/URI/extensions; a hand-built
//! [`ToolCall`](crate::ToolCall) without parts falls back to the same
//! neutral statics guard unit tests use.
//!
//! A guard rejection (an HTTP [`Response`]) is folded back into an
//! [`McpError`] by status via [`guard_response_to_error`].

use std::net::SocketAddr;

use r2e_core::http::body::to_bytes;
use r2e_core::http::{ConnectInfo, Response, Uri};
use r2e_core::{default_method, no_extensions, GuardContext, Identity, PathParams};

use crate::error::{from_status, McpError};

/// Cap on how much of a guard rejection body is read back into the JSON-RPC
/// error message.
const REJECTION_BODY_LIMIT: usize = 64 * 1024;

fn default_uri() -> &'static Uri {
    static URI: std::sync::LazyLock<Uri> = std::sync::LazyLock::new(|| Uri::from_static("/"));
    &URI
}

/// Build a [`GuardContext`] from optional transport parts — the shared form
/// used by the generated tool, resource and prompt dispatch (their calls all
/// carry the same `parts` field).
pub fn member_guard_context<'a, I: Identity>(
    parts: Option<&'a r2e_core::http::Parts>,
    method_name: &'static str,
    controller_name: &'static str,
    identity: Option<&'a I>,
) -> GuardContext<'a, I> {
    static EMPTY_HEADERS: std::sync::LazyLock<r2e_core::http::HeaderMap> =
        std::sync::LazyLock::new(r2e_core::http::HeaderMap::new);
    match parts {
        Some(parts) => GuardContext {
            method_name,
            controller_name,
            method: &parts.method,
            headers: &parts.headers,
            uri: &parts.uri,
            extensions: &parts.extensions,
            peer_addr: parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
            path_params: PathParams::EMPTY,
            identity,
        },
        None => GuardContext {
            method_name,
            controller_name,
            method: default_method(),
            headers: &EMPTY_HEADERS,
            uri: default_uri(),
            extensions: no_extensions(),
            peer_addr: None,
            path_params: PathParams::EMPTY,
            identity,
        },
    }
}

/// Fold a guard rejection [`Response`] into an [`McpError`] by status:
/// 401 → [`Unauthorized`](McpError::Unauthorized), 403 →
/// [`Forbidden`](McpError::Forbidden), 404 → [`NotFound`](McpError::NotFound),
/// 400/422 → [`InvalidParams`](McpError::InvalidParams), 5xx →
/// [`Internal`](McpError::Internal), anything else a domain
/// [`Tool`](McpError::Tool) failure. The response body (typically the guard's
/// JSON error payload) becomes the message.
pub async fn guard_response_to_error(response: Response) -> McpError {
    let (parts, body) = response.into_parts();
    let status = parts.status.as_u16();
    let message = match to_bytes(body, REJECTION_BODY_LIMIT).await {
        Ok(bytes) if !bytes.is_empty() => String::from_utf8_lossy(&bytes).into_owned(),
        _ => format!("request rejected with status {status}"),
    };
    from_status(status, message)
}
