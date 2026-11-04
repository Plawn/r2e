//! `#[routes]`: one impl macro for the members a type serves over an
//! HTTP-hosted port — HTTP routes (`#[get]`/…/`#[sse]`/`#[ws]`/`#[fallback]`,
//! plus the transverse `#[consumer]`/`#[scheduled]`/`#[async_exec]` and
//! lifecycle hooks) or MCP members (`#[tool]`/`#[resource]`/`#[prompt]`/
//! `#[completion]`).
//!
//! One type exposes one port: the port is inferred from the members and each
//! shape reuses its transport's parser and codegen unchanged:
//!
//! - **HTTP** (no MCP member) → `routes_parsing` + `codegen` (a
//!   `Controller<S, W>`). An impl with no member at all reads as HTTP.
//! - **MCP** (MCP members, no HTTP route) → `mcp_routes_parsing` +
//!   `mcp_codegen` (an `McpService<S, W>`, no `Controller`).
//!
//! MCP members next to HTTP routes in one impl are a compile error: the MCP
//! members belong on their own `#[controller]` type over the same beans.

use proc_macro2::TokenStream;

use crate::codegen;
use crate::extract::route::{is_fallback_attr, is_route_attr};
use crate::mcp_codegen;
use crate::parsing::mcp_routes_parsing;
use crate::parsing::member::MCP_MEMBER_MARKERS;
use crate::parsing::routes_parsing;

/// An HTTP route member: a verb route, `#[sse]`, `#[ws]` or `#[fallback]`.
fn http_route_marker(attr: &syn::Attribute) -> bool {
    is_route_attr(attr)
        || is_fallback_attr(attr)
        || attr.path().is_ident("sse")
        || attr.path().is_ident("ws")
}

fn mcp_marker(attr: &syn::Attribute) -> bool {
    MCP_MEMBER_MARKERS.iter().any(|m| attr.path().is_ident(m))
}

/// Expand `#[routes(args)]`.
pub fn expand(args: TokenStream, input: proc_macro::TokenStream) -> TokenStream {
    let item = match syn::parse::<syn::ItemImpl>(input) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };
    match expand_impl(args, item) {
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error(),
    }
}

fn expand_impl(args: TokenStream, item: syn::ItemImpl) -> syn::Result<TokenStream> {
    let group = mcp_routes_parsing::parse_impl_args(args.clone())?;

    let methods = || {
        item.items.iter().filter_map(|impl_item| match impl_item {
            syn::ImplItem::Fn(method) => Some(method),
            _ => None,
        })
    };
    let first_mcp = methods().flat_map(|m| &m.attrs).find(|a| mcp_marker(a));
    let has_http = methods().flat_map(|m| &m.attrs).any(http_route_marker);

    if let Some(mcp_attr) = first_mcp
        && has_http
    {
        return Err(syn::Error::new_spanned(
            mcp_attr,
            "`#[tool]`, `#[resource]`, `#[prompt]` and `#[completion]` cannot share an \
             impl with HTTP routes: one type exposes one port. Move the MCP members to \
             their own `#[controller]` type over the same beans \
             (docs/features/25-mcp.md § One port per type).",
        ));
    }

    if !has_http && first_mcp.is_some() {
        let def = mcp_routes_parsing::parse(item, group)?;
        return Ok(mcp_codegen::generate(&def));
    }

    if group.is_some() {
        return Err(syn::Error::new_spanned(
            &args,
            "`group`/`opt_in` apply to MCP members — this #[routes] impl has no \
             #[tool]/#[resource]/#[prompt]/#[completion] member",
        ));
    }
    let def = routes_parsing::parse(item)?;
    Ok(codegen::generate(&def))
}
