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
//! A guard [`Rejection`](r2e_core::Rejection) is projected onto an
//! [`McpError`](crate::McpError) **by kind** through
//! `From<Rejection> for McpError` (see `error.rs`) — never by re-reading an
//! HTTP status or response body.

use std::net::SocketAddr;

use r2e_core::http::{ConnectInfo, Uri};
use r2e_core::{default_method, no_extensions, GuardContext, Identity, PathParams};

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
