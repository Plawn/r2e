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

use r2e_core::http::Parts;
use r2e_core::{GuardContext, Identity, PathParams};

/// The head a hand-built call without transport parts is guarded against:
/// `GET /`, no headers, no extensions, no peer address — the neutral values
/// guard unit tests use.
fn empty_parts() -> &'static Parts {
    static PARTS: std::sync::LazyLock<Parts> = std::sync::LazyLock::new(|| {
        r2e_core::http::Request::new(r2e_core::http::Body::empty())
            .into_parts()
            .0
    });
    &PARTS
}

/// Build a [`GuardContext`] from optional transport parts — the shared form
/// used by the generated tool, resource and prompt dispatch (their calls all
/// carry the same `parts` field). A thin call to
/// [`GuardContext::from_parts`]; a call without parts is guarded against an
/// empty head.
pub fn member_guard_context<'a, I: Identity>(
    parts: Option<&'a Parts>,
    method_name: &'static str,
    controller_name: &'static str,
    identity: Option<&'a I>,
) -> GuardContext<'a, I> {
    let parts: &'a Parts = match parts {
        Some(parts) => parts,
        None => empty_parts(),
    };
    GuardContext::from_parts(
        parts,
        method_name,
        controller_name,
        identity,
        PathParams::EMPTY,
    )
}
