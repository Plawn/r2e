//! `#[routes]`: one impl macro for every member a type serves over HTTP-hosted
//! transports — HTTP routes (`#[get]`/…/`#[sse]`/`#[ws]`, plus the
//! transverse `#[consumer]`/`#[scheduled]`/`#[async_exec]` and lifecycle
//! hooks) and MCP members (`#[tool]`/`#[resource]`/`#[prompt]`/
//! `#[completion]`).
//!
//! The impl is classified up front and each shape reuses its transport's
//! parser and codegen unchanged:
//!
//! - **HTTP-only** → `routes_parsing` + `codegen` (a `Controller<S, W>`).
//! - **MCP-only** → `mcp_routes_parsing` + `mcp_codegen` (an
//!   `McpService<S, W>`, no `Controller`).
//! - **Mixed** → the impl is split by member: the HTTP side (routes, helpers,
//!   associated items, every impl-level decorator) and the MCP side (the
//!   members, under the impl-level decorators both transports share —
//!   `#[guard]`/`#[roles]`/`#[all_roles]`/`#[intercept]`; `#[pre_guard]`/
//!   `#[middleware]`/`#[layer]` stay HTTP-only). Each side is parsed by its
//!   own parser, then the HTTP codegen emits the one façade impl (routes AND
//!   members) and the one `EndpointDeps` carrier (both site sets), and the MCP
//!   codegen emits its wrapper and `McpService` impl beside the `Controller`
//!   impl. Each trait keeps its own witness.
//!
//! The deprecated `#[mcp_routes]` alias expands through the same dispatcher.

use proc_macro2::TokenStream;
use quote::ToTokens;

use crate::codegen;
use crate::extract::route::{is_fallback_attr, is_route_attr};
use crate::mcp_codegen;
use crate::parsing::mcp_routes_parsing;
use crate::parsing::member::MCP_MEMBER_MARKERS;
use crate::parsing::routes_parsing::{self, MixedMcp};

/// Markers that make a method an HTTP-side member: anything `#[routes]` wires
/// into the `Controller` impl.
const HTTP_SIDE_MARKERS: &[&str] = &[
    "sse",
    "ws",
    "consumer",
    "scheduled",
    "async_exec",
    "post_construct",
    "pre_destroy",
    "on_start",
];

/// HTTP-side markers that `register_controller` alone wires (lifecycle hooks
/// and background members): a mixed type declaring one must be registered
/// with `register_controller`, not only with `register_mcp_service`.
const CONTROLLER_HOSTED_MARKERS: &[&str] = &[
    "post_construct",
    "on_start",
    "pre_destroy",
    "scheduled",
    "consumer",
    "async_exec",
];

/// Impl-level decorators that only the HTTP pipeline has: never applied to
/// MCP members of a mixed impl.
const HTTP_ONLY_IMPL_DECORATORS: &[&str] = &["pre_guard", "middleware", "layer"];

fn http_side_marker(attr: &syn::Attribute) -> bool {
    is_route_attr(attr)
        || is_fallback_attr(attr)
        || HTTP_SIDE_MARKERS.iter().any(|m| attr.path().is_ident(m))
}

fn mcp_marker(attr: &syn::Attribute) -> bool {
    MCP_MEMBER_MARKERS.iter().any(|m| attr.path().is_ident(m))
}

fn attr_name(attr: &syn::Attribute) -> String {
    attr.path()
        .get_ident()
        .map(|i| i.to_string())
        .unwrap_or_else(|| attr.path().to_token_stream().to_string())
}

/// `true` when the method carries an MCP member marker. Errors when it also
/// carries an HTTP-side marker, spanned at whichever of the two comes second.
fn classify_method(method: &syn::ImplItemFn) -> syn::Result<bool> {
    let mut http: Option<&syn::Attribute> = None;
    let mut mcp: Option<&syn::Attribute> = None;
    for attr in &method.attrs {
        let is_http = http_side_marker(attr);
        let is_mcp = mcp_marker(attr);
        let other = if is_http {
            mcp
        } else if is_mcp {
            http
        } else {
            continue;
        };
        if let Some(first) = other {
            return Err(syn::Error::new_spanned(
                attr,
                format!(
                    "a method cannot be both #[{}] and #[{}] — an HTTP member and an MCP \
                     member are separate methods: give each its own method and share the \
                     logic through a helper (or a bean)",
                    attr_name(first),
                    attr_name(attr)
                ),
            ));
        }
        if is_http && http.is_none() {
            http = Some(attr);
        }
        if is_mcp && mcp.is_none() {
            mcp = Some(attr);
        }
    }
    Ok(mcp.is_some())
}

/// Expand `#[routes(args)]` (`attr = "routes"`) or the deprecated
/// `#[mcp_routes(args)]` alias (`attr = "mcp_routes"`).
pub fn expand(args: TokenStream, input: proc_macro::TokenStream, attr: &str) -> TokenStream {
    let item = match syn::parse::<syn::ItemImpl>(input) {
        Ok(item) => item,
        Err(err) => return err.to_compile_error(),
    };
    match expand_impl(args, item, attr) {
        Ok(tokens) => tokens,
        Err(err) => err.to_compile_error(),
    }
}

fn expand_impl(args: TokenStream, item: syn::ItemImpl, attr: &str) -> syn::Result<TokenStream> {
    let group = mcp_routes_parsing::parse_impl_args(args.clone(), attr)?;

    let mut has_mcp = false;
    let mut has_http = false;
    for impl_item in &item.items {
        if let syn::ImplItem::Fn(method) = impl_item {
            if classify_method(method)? {
                has_mcp = true;
            } else if method.attrs.iter().any(http_side_marker) {
                has_http = true;
            }
        }
    }

    // The alias keeps its MCP-only reading of an impl with no member at all
    // (an empty service); `#[routes]` keeps its HTTP reading.
    let mcp_only = !has_http && (has_mcp || attr == "mcp_routes");
    if mcp_only {
        let def = mcp_routes_parsing::parse(item, group)?;
        return Ok(mcp_codegen::generate(&def));
    }

    if !has_mcp {
        if group.is_some() {
            return Err(syn::Error::new_spanned(
                &args,
                format!(
                    "`group`/`opt_in` apply to MCP members — this #[{attr}] impl has no \
                     #[tool]/#[resource]/#[prompt]/#[completion] member"
                ),
            ));
        }
        let def = routes_parsing::parse(item, false)?;
        return Ok(codegen::generate(&def));
    }

    // Mixed: split the members off, parse each side with its own parser.
    let mut http_item = item;
    let mut mcp_item = http_item.clone();
    let (mcp_members, http_items): (Vec<_>, Vec<_>) =
        std::mem::take(&mut http_item.items).into_iter().partition(
            |impl_item| matches!(impl_item, syn::ImplItem::Fn(m) if m.attrs.iter().any(mcp_marker)),
        );
    http_item.items = http_items;
    mcp_item.items = mcp_members;
    mcp_item.attrs.retain(|a| {
        !HTTP_ONLY_IMPL_DECORATORS
            .iter()
            .any(|d| a.path().is_ident(d))
    });

    let controller_hosted_markers: Vec<String> = CONTROLLER_HOSTED_MARKERS
        .iter()
        .filter(|marker| {
            http_item.items.iter().any(|impl_item| {
                matches!(impl_item, syn::ImplItem::Fn(m)
                    if m.attrs.iter().any(|a| a.path().is_ident(marker)))
            })
        })
        .map(|marker| format!("#[{marker}]"))
        .collect();

    let mut http_def = routes_parsing::parse(http_item, true)?;
    let mut mcp_def = mcp_routes_parsing::parse(mcp_item, group)?;

    let http_sites: Vec<syn::Expr> = codegen::decorators::controller_site_exprs(&http_def)
        .into_iter()
        .cloned()
        .collect();
    http_def.mcp = Some(MixedMcp {
        facade_methods: mcp_def.facade_methods.clone(),
        site_exprs: mcp_codegen::service_impl::site_exprs(&mcp_def)
            .into_iter()
            .cloned()
            .collect(),
    });
    mcp_def.mixed_http_sites = Some(http_sites);
    mcp_def.controller_hosted_markers = controller_hosted_markers;

    let http = codegen::generate(&http_def);
    let mcp = mcp_codegen::generate(&mcp_def);
    Ok(quote::quote! {
        #http
        #mcp
    })
}
