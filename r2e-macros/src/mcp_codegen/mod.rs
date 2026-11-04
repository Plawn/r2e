//! Code generation for a `#[routes]` impl block with MCP members.
//!
//! Generates:
//! - The user's impl, split exactly like `#[routes]` splits an HTTP
//!   controller: members and `#[request_helper]` helpers on the request
//!   façade `impl __R2eRequest_<Name>` (where `self.<identity field>` /
//!   `self.<request field>` resolve), everything else on the core `impl Name`
//! - A wrapper struct `__R2eMcp<Name>` holding the `Arc`'d core and the
//!   prebuilt guard/interceptor sets (`MemberDecoSets`, embedded by value);
//!   the wrapper itself is shared through one Arc
//! - One hidden invocation method per member on the wrapper
//!   (`__r2e_tool_<fn>` / `__r2e_resource_<fn>` / `__r2e_prompt_<fn>`):
//!   scope check → façade binding (the request data extracted once per HTTP
//!   request by the endpoint layer) → parameter identity extraction → guards
//!   → params → method call (interceptor-wrapped) → family-specific result
//!   conversion
//! - A state-generic impl of `McpService<S, W>` for the controller
//!   (`routes()` bundling tool, resource and prompt routes with schemas, plus
//!   the once-per-request extraction of the struct's request scope); the
//!   request-data marker and one extraction marker per member
//!   `#[inject(identity)]` are folded into `W`, exactly like
//!   `Controller<S, W>` for HTTP routes
//! - An impl of `EndpointDeps` for the controller (compile-time bean check
//!   at `register_mcp_service()`)

mod service_impl;
mod tool_impl;

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;

use crate::codegen::decorators::MemberDecoSets;
use crate::parsing::mcp_routes_parsing::{McpMemberKind, McpRoutesImplDef, McpTool};

/// The hidden wrapper struct ident for a service.
pub(crate) fn wrapper_ident(controller_name: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eMcp{}", controller_name)
}

/// The `#[controller]`-generated items a service is wired through: the meta
/// module, the request-data type and its marker generic, the façade.
pub(crate) struct ControllerItems {
    pub meta_mod: syn::Ident,
    pub data_name: syn::Ident,
    pub data_marker: syn::Ident,
    pub facade_name: syn::Ident,
}

pub(crate) fn controller_items(controller_name: &syn::Ident) -> ControllerItems {
    ControllerItems {
        meta_mod: format_ident!("__r2e_meta_{}", controller_name),
        data_name: format_ident!("__R2eRequestData_{}", controller_name),
        // The same spelling `#[routes]` uses (`handlers::data_marker`).
        data_marker: format_ident!("__R2eMd"),
        facade_name: format_ident!("__R2eRequest_{}", controller_name),
    }
}

/// The hidden per-member invocation method ident on the wrapper, prefixed by
/// family so a method name can never collide across kinds.
pub(crate) fn invoke_ident(kind: McpMemberKind, fn_name: &syn::Ident) -> syn::Ident {
    format_ident!("__r2e_{}_{}", kind.attr_name(), fn_name)
}

/// A member's `#[inject(identity)]` extraction site: the opaque marker
/// generic (`__R2eMp<Fn>`, inferred at `register_mcp_service`) and the
/// declared parameter type (`I` or `Option<I>`), extracted through
/// `FromRequestPartsVia<S, marker>` — the same contract as an HTTP route's
/// identity parameter.
pub(crate) struct IdentitySite {
    pub marker: syn::Ident,
    pub declared_ty: TokenStream,
}

/// The identity extraction site of a member, if it declares one.
pub(crate) fn identity_site(tool: &McpTool) -> Option<IdentitySite> {
    let p = tool.identity_param()?;
    let ty = &p.ty;
    let declared_ty = if p.is_optional {
        quote_spanned! {ty.span()=> ::core::option::Option<#ty> }
    } else {
        quote! { #ty }
    };
    Some(IdentitySite {
        marker: crate::codegen::handlers::identity_marker_for(&tool.member.name),
        declared_ty,
    })
}

/// The `ToolRequirements` struct-literal expression for one member:
/// marker-level `scopes`/`any_scopes` plus the recorded `#[roles]`/
/// `#[all_roles]` literals (impl-level first, then method-level). Roles are
/// ENFORCED by the generated guard; they are recorded here so the list
/// endpoints can filter. All-`'static` literal slices, so the expression is
/// const-compatible.
pub(crate) fn requirements_expr(
    def: &McpRoutesImplDef,
    tool: &McpTool,
    mcp: &TokenStream,
) -> TokenStream {
    let scopes = &tool.meta.scopes;
    let any_scopes = &tool.meta.any_scopes;
    let roles: Vec<&String> = def
        .controller_roles
        .iter()
        .chain(tool.member.decorators.roles.iter())
        .collect();
    let all_roles: Vec<&String> = def
        .controller_all_roles
        .iter()
        .chain(tool.member.decorators.all_roles.iter())
        .collect();
    quote! {
        #mcp::__macro_support::ToolRequirements {
            scopes: &[#(#scopes),*],
            any_scopes: &[#(#any_scopes),*],
            roles: &[#(#roles),*],
            all_roles: &[#(#all_roles),*],
        }
    }
}

/// Build the decorator sets embedded in the wrapper: the shared
/// controller-level set (impl-level sites, built only when at least one
/// member exists — this MUST mirror `service_impl::site_exprs`, which feeds
/// the `EndpointDeps` fold and `validate_config`: a product built here
/// without its deps folded there turns a missing bean into a boot panic
/// instead of a compile error at `register_mcp_service`) plus one set per
/// member.
fn build_deco_sets(def: &McpRoutesImplDef) -> MemberDecoSets {
    MemberDecoSets::build(
        &def.controller_name,
        "Mcp",
        &def.controller_guards,
        &def.controller_intercepts,
        def.members.iter().map(|m| &m.member),
    )
}

/// Main entry point: generate the MCP code of a `#[routes]` impl block.
pub fn generate(def: &McpRoutesImplDef) -> TokenStream {
    let deco = build_deco_sets(def);
    let impl_block = generate_impl_block(def);
    let wrapper = generate_wrapper_struct(def, &deco);
    let invoke_impl = tool_impl::generate_invoke_impl(def, &deco);
    let mcp_service_impl = service_impl::generate_mcp_service_impl(def, &deco);
    let endpoint_deps_impl = service_impl::generate_endpoint_deps_impl(def);
    let deco_items = &deco.items;

    quote! {
        #impl_block
        #deco_items
        #wrapper
        #invoke_impl
        #mcp_service_impl
        #endpoint_deps_impl
    }
}

/// Generate the user's impl, split by execution scope like `#[routes]`
/// does: members and `#[request_helper]` helpers on the request façade,
/// everything else (helpers, associated items) on the core. The impl-level
/// attributes are carried by both blocks (`validate_mcp_impl_attrs` only
/// lets inert ones through, so emitting them twice is sound).
fn generate_impl_block(def: &McpRoutesImplDef) -> TokenStream {
    let impl_block = &def.impl_block;
    let core_impl = if impl_block.items.is_empty() {
        quote! {}
    } else {
        quote! { #impl_block }
    };
    let facade_impl = if def.facade_methods.is_empty() {
        quote! {}
    } else {
        let facade_name = controller_items(&def.controller_name).facade_name;
        let impl_attrs = &impl_block.attrs;
        let methods = def.facade_methods.iter().map(|f| {
            let mut f = f.clone();
            crate::codegen::precise_capture::add_handler_precise_captures(&mut f.sig);
            quote! { #f }
        });
        quote! {
            #(#impl_attrs)*
            impl #facade_name {
                #(#methods)*
            }
        }
    };
    quote! {
        #facade_impl
        #core_impl
    }
}

/// Generate the single wrapper struct holding the controller core and every
/// prebuilt decorator product.
///
/// `routes()` puts this wrapper behind one Arc. The decorator sets are
/// embedded by value (no nested deco Arc): a call borrows
/// `&self.__deco_<fn>.__g0` straight through the wrapper.
fn generate_wrapper_struct(def: &McpRoutesImplDef, deco: &MemberDecoSets) -> TokenStream {
    let controller_name = &def.controller_name;
    let wrapper_name = wrapper_ident(controller_name);
    let field_decls = deco.field_decls();

    quote! {
        #[doc(hidden)]
        pub struct #wrapper_name {
            // Shared with every façade bound per call (`bind_request` takes
            // an `Arc` of the core), like an HTTP controller's core.
            core: ::std::sync::Arc<#controller_name>,
            #(#field_decls,)*
        }
    }
}
