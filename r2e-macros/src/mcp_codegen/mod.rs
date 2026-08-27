//! Code generation for the `#[mcp_routes]` attribute macro.
//!
//! Generates:
//! - The user's impl block (methods with stripped attributes)
//! - A wrapper struct `__R2eMcp<Name>` holding the shared core and the
//!   prebuilt per-member guard/interceptor sets
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

use crate::codegen::decorators::{generate_named_deco_items, DecoSet};
use crate::parsing::mcp_routes_parsing::{McpMemberKind, McpRoutesImplDef, McpTool};

/// Per-member prebuilt decorator sets for an MCP impl block.
///
/// `sets` is parallel to `def.members`: `None` when the member has no
/// guard/interceptor sites (or when spec inference failed — the
/// `compile_error!` then lives in `items` and the member degrades to the
/// unwrapped shape).
///
/// All sets live in one hidden container struct behind a single `Arc` on the
/// wrapper, so cloning the wrapper per invoke closure costs one ref-count
/// bump.
pub(crate) struct McpDecoSets {
    pub items: TokenStream,
    sets: Vec<Option<DecoSet>>,
}

impl McpDecoSets {
    /// The hidden container struct holding every member's prebuilt set.
    pub fn container_ident(controller_name: &syn::Ident) -> syn::Ident {
        format_ident!("__R2eMcpDecos_{}", controller_name)
    }

    /// The container field holding one member's prebuilt set.
    pub fn field_ident(fn_name: &syn::Ident) -> syn::Ident {
        format_ident!("__deco_{}", fn_name)
    }

    /// Whether any member has a prebuilt set (i.e. the container exists).
    pub fn has_any(&self) -> bool {
        self.sets.iter().any(Option::is_some)
    }

    /// The set for one member, positionally paired with `def.members`.
    pub fn set_for(&self, index: usize) -> Option<&DecoSet> {
        self.sets[index].as_ref()
    }

    /// `(container field, set)` for every decorated member, in `def.members`
    /// order.
    pub fn fields<'a>(
        &'a self,
        def: &'a McpRoutesImplDef,
    ) -> impl Iterator<Item = (syn::Ident, &'a DecoSet)> {
        def.members
            .iter()
            .zip(self.sets.iter())
            .filter_map(|(t, set)| set.as_ref().map(|s| (Self::field_ident(&t.name), s)))
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

/// The guard expressions of one member, controller-level first (impl-level
/// `#[roles]`/`#[all_roles]`/`#[guard]` run before method-level ones) —
/// mirroring HTTP route ordering. Controller-level guard products are
/// duplicated per member (each member's set builds its own instance),
/// unlike HTTP's shared controller set; acceptable because MCP impl-level
/// guards are rare and stateless-by-convention. Documented in the feature
/// guide.
fn tool_guard_exprs(def: &McpRoutesImplDef, tool: &McpTool) -> Vec<syn::Expr> {
    def.controller_guards
        .iter()
        .chain(tool.decorators.guard_fns.iter())
        .cloned()
        .collect()
}

/// Build the decorator sets (hidden struct + ctor per member) from the guard
/// and interceptor sites. Controller-level interceptors first, then
/// method-level — same execution order as HTTP routes / gRPC methods.
fn build_deco_sets(def: &McpRoutesImplDef) -> McpDecoSets {
    let mut items = quote! {};
    let mut sets = Vec::with_capacity(def.members.len());
    for tool in &def.members {
        let guard_exprs = tool_guard_exprs(def, tool);
        let intercept_exprs: Vec<&syn::Expr> = def
            .controller_intercepts
            .iter()
            .chain(tool.decorators.intercept_fns.iter())
            .collect();
        let (tool_items, set) = generate_named_deco_items(
            &def.controller_name,
            "McpDeco",
            &tool.name,
            &guard_exprs,
            &intercept_exprs,
            quote! {},
        );
        items.extend(tool_items);
        sets.push(set);
    }
    McpDecoSets { items, sets }
}

/// Main entry point: generate all code for an `#[mcp_routes]` impl block.
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

/// Generate the user's impl block with member attributes stripped.
fn generate_impl_block(def: &McpRoutesImplDef) -> TokenStream {
    let controller_name = &def.controller_name;

    let methods: Vec<&syn::ImplItemFn> = def
        .members
        .iter()
        .map(|t| &t.fn_item)
        .chain(def.other_methods.iter())
        .collect();

    quote! {
        impl #controller_name {
            #(#methods)*
        }
    }
}

/// Generate the wrapper struct holding the controller core + the prebuilt
/// decorator-set container, plus the container struct itself.
///
/// The core is built ONCE from the bean graph (`ContextConstruct`) when the
/// service is registered; invoke closures share it through the `Arc`.
/// Guard/interceptor sets are built at the same time, from the same context
/// (`DecoratorSpec::build`) — never per call.
fn generate_wrapper_struct(def: &McpRoutesImplDef, deco: &McpDecoSets) -> TokenStream {
    let controller_name = &def.controller_name;
    let wrapper_name = wrapper_ident(controller_name);

    let (container_decl, decos_field) = if deco.has_any() {
        let container = McpDecoSets::container_ident(controller_name);
        let fields: Vec<TokenStream> = deco
            .fields(def)
            .map(|(field, set)| {
                let ty = set.ty();
                quote! { #field: #ty }
            })
            .collect();
        (
            quote! {
                #[allow(non_camel_case_types)]
                #[doc(hidden)]
                struct #container {
                    #(#fields,)*
                }
            },
            quote! { __decos: ::std::sync::Arc<#container>, },
        )
    } else {
        (quote! {}, quote! {})
    };

    quote! {
        #container_decl

        #[doc(hidden)]
        #[derive(Clone)]
        pub struct #wrapper_name {
            core: ::std::sync::Arc<#controller_name>,
            #decos_field
        }
    }
}
