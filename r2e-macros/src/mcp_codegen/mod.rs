//! Code generation for the `#[mcp_routes]` attribute macro.
//!
//! Generates:
//! - The user's impl block (methods with stripped attributes)
//! - A wrapper struct `__R2eMcp<Name>` holding the core and every prebuilt
//!   guard/interceptor product; the wrapper itself is shared through one Arc
//! - One hidden invocation method per member on the wrapper
//!   (`__r2e_tool_<fn>` / `__r2e_resource_<fn>` / `__r2e_prompt_<fn>`):
//!   scope check → identity extraction → guards → params → method call
//!   (interceptor-wrapped) → family-specific result conversion
//! - An impl of `McpService` for the controller (`routes()` bundling tool,
//!   resource and prompt routes with schemas)
//! - An impl of `EndpointDeps` for the controller (compile-time bean check
//!   at `register_mcp_service()`)

mod service_impl;
mod tool_impl;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::codegen::decorators::{decorator_product_field, spec_type_of};
use crate::parsing::mcp_routes_parsing::{McpMemberKind, McpRoutesImplDef, McpTool};

#[derive(Default)]
pub(crate) struct McpMemberDecos {
    pub guard_fields: Vec<syn::Ident>,
    pub intercept_fields: Vec<syn::Ident>,
}

/// Flat decorator layout embedded directly in the single MCP wrapper.
///
/// A flat layout keeps the generated surface compact: no per-member structs,
/// constructors, or nested Arcs. Controller-level products have one field and
/// are shared by every member; method-level products keep one field per site.
pub(crate) struct McpDecoLayout {
    pub items: TokenStream,
    pub field_decls: Vec<TokenStream>,
    pub field_inits: Vec<TokenStream>,
    pub controller_guard_fields: Vec<syn::Ident>,
    pub controller_intercept_fields: Vec<syn::Ident>,
    members: Vec<McpMemberDecos>,
}

impl McpDecoLayout {
    pub fn member(&self, index: usize) -> &McpMemberDecos {
        &self.members[index]
    }
}

/// The hidden wrapper struct ident for a service.
pub(crate) fn wrapper_ident(controller_name: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eMcp{}", controller_name)
}

/// The hidden per-member invocation method ident on the wrapper, prefixed by
/// family so a method name can never collide across kinds.
pub(crate) fn invoke_ident(kind: McpMemberKind, fn_name: &syn::Ident) -> syn::Ident {
    format_ident!("__r2e_{}_{}", kind.attr_name(), fn_name)
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
        .chain(tool.decorators.roles.iter())
        .collect();
    let all_roles: Vec<&String> = def
        .controller_all_roles
        .iter()
        .chain(tool.decorators.all_roles.iter())
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

/// Build one flat decorator layout for the wrapper. Controller-level sites
/// are emitted once, followed by each member's own sites.
fn build_deco_layout(def: &McpRoutesImplDef) -> McpDecoLayout {
    let all_sites = def
        .controller_intercepts
        .iter()
        .chain(def.controller_guards.iter())
        .chain(def.members.iter().flat_map(|m| {
            m.decorators
                .guard_fns
                .iter()
                .chain(&m.decorators.intercept_fns)
        }));
    let mut items = TokenStream::new();
    for expr in all_sites {
        if let Err(err) = spec_type_of(expr) {
            items.extend(err.to_compile_error());
        }
    }
    if !items.is_empty() {
        return McpDecoLayout {
            items,
            field_decls: Vec::new(),
            field_inits: Vec::new(),
            controller_guard_fields: Vec::new(),
            controller_intercept_fields: Vec::new(),
            members: (0..def.members.len())
                .map(|_| McpMemberDecos::default())
                .collect(),
        };
    }

    let mut field_decls = Vec::new();
    let mut field_inits = Vec::new();
    let mut add_site = |field: syn::Ident, expr: &syn::Expr| {
        let (field_decl, field_init) =
            decorator_product_field(&field, expr).expect("decorator specs prevalidated");
        field_decls.push(field_decl);
        field_inits.push(field_init);
        field
    };

    let controller_intercept_fields = def
        .controller_intercepts
        .iter()
        .enumerate()
        .map(|(i, expr)| add_site(format_ident!("__ctrl_i{}", i), expr))
        .collect();
    let controller_guard_fields = def
        .controller_guards
        .iter()
        .enumerate()
        .map(|(i, expr)| add_site(format_ident!("__ctrl_g{}", i), expr))
        .collect();
    let members = def
        .members
        .iter()
        .enumerate()
        .map(|(member_index, member)| McpMemberDecos {
            guard_fields: member
                .decorators
                .guard_fns
                .iter()
                .enumerate()
                .map(|(i, expr)| add_site(format_ident!("__m{}_g{}", member_index, i), expr))
                .collect(),
            intercept_fields: member
                .decorators
                .intercept_fns
                .iter()
                .enumerate()
                .map(|(i, expr)| add_site(format_ident!("__m{}_i{}", member_index, i), expr))
                .collect(),
        })
        .collect();

    McpDecoLayout {
        items,
        field_decls,
        field_inits,
        controller_guard_fields,
        controller_intercept_fields,
        members,
    }
}

/// Main entry point: generate all code for an `#[mcp_routes]` impl block.
pub fn generate(def: &McpRoutesImplDef) -> TokenStream {
    let deco = build_deco_layout(def);
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

/// Generate the user's impl block with member attributes stripped.
fn generate_impl_block(def: &McpRoutesImplDef) -> TokenStream {
    let impl_block = &def.impl_block;
    quote! { #impl_block }
}

/// Generate the single wrapper struct holding the controller core and every
/// prebuilt decorator product.
///
/// `routes()` puts this wrapper behind one Arc. There are no nested core/deco
/// Arcs and no per-member generated structs.
fn generate_wrapper_struct(def: &McpRoutesImplDef, deco: &McpDecoLayout) -> TokenStream {
    let controller_name = &def.controller_name;
    let wrapper_name = wrapper_ident(controller_name);
    let field_decls = &deco.field_decls;

    quote! {
        #[doc(hidden)]
        pub struct #wrapper_name {
            core: #controller_name,
            #(#field_decls,)*
        }
    }
}
