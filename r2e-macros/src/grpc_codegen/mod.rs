//! Code generation for the `#[grpc_routes]` attribute macro.
//!
//! Generates:
//! - The user's impl block (methods with stripped attributes)
//! - A wrapper struct `__R2eGrpc<Name>` that holds the shared core and the
//!   prebuilt per-method interceptor sets
//! - An impl of the tonic-generated trait for the wrapper
//! - An impl of `GrpcService<T>` for the controller

mod service_impl;
mod trait_impl;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::codegen::decorators::{generate_named_deco_items, DecoSet};
use crate::grpc_routes_parsing::GrpcRoutesImplDef;

/// Per-method prebuilt interceptor sets for a gRPC impl block.
///
/// `sets` is parallel to `def.methods`: `None` when the method has no
/// interceptor sites (or when spec inference failed — the `compile_error!`
/// then lives in `items` and the method degrades to the unwrapped shape).
///
/// All sets live in one hidden container struct behind a single `Arc` on the
/// wrapper (`__decos`), so cloning the wrapper (tonic clones the service per
/// call) costs one ref-count bump regardless of how many methods are
/// intercepted.
pub(crate) struct GrpcDecoSets {
    pub items: TokenStream,
    sets: Vec<Option<DecoSet>>,
}

impl GrpcDecoSets {
    /// The hidden container struct holding every method's prebuilt set.
    pub fn container_ident(controller_name: &syn::Ident) -> syn::Ident {
        format_ident!("__R2eGrpcDecos_{}", controller_name)
    }

    /// The container field holding one method's prebuilt set.
    pub fn field_ident(fn_name: &syn::Ident) -> syn::Ident {
        format_ident!("__deco_{}", fn_name)
    }

    /// Whether any method has a prebuilt set (i.e. the container exists).
    pub fn has_any(&self) -> bool {
        self.sets.iter().any(Option::is_some)
    }

    /// The set for one method, positionally paired with `def.methods`.
    pub fn set_for(&self, index: usize) -> Option<&DecoSet> {
        self.sets[index].as_ref()
    }

    /// `(container field, set)` for every intercepted method, in
    /// `def.methods` order — the single source of the method ↔ field
    /// pairing shared by the container decl, its init, and the trait impl.
    pub fn fields<'a>(
        &'a self,
        def: &'a GrpcRoutesImplDef,
    ) -> impl Iterator<Item = (syn::Ident, &'a DecoSet)> {
        def.methods
            .iter()
            .zip(self.sets.iter())
            .filter_map(|(m, set)| set.as_ref().map(|s| (Self::field_ident(&m.name), s)))
    }
}

/// Build the decorator sets (hidden struct + ctor per method) from the
/// interceptor sites. Controller-level interceptors first, then
/// method-level — same execution order as HTTP routes and scheduled tasks.
fn build_deco_sets(def: &GrpcRoutesImplDef) -> GrpcDecoSets {
    let mut items = quote! {};
    let mut sets = Vec::with_capacity(def.methods.len());
    for method in &def.methods {
        let intercept_exprs: Vec<&syn::Expr> = def
            .controller_intercepts
            .iter()
            .chain(method.decorators.intercept_fns.iter())
            .collect();
        let (method_items, set) = generate_named_deco_items(
            &def.controller_name,
            "GrpcDeco",
            &method.name,
            &[],
            &intercept_exprs,
            quote! {},
        );
        items.extend(method_items);
        sets.push(set);
    }
    GrpcDecoSets { items, sets }
}

/// Main entry point: generate all code for a `#[grpc_routes]` impl block.
pub fn generate(def: &GrpcRoutesImplDef) -> TokenStream {
    let deco = build_deco_sets(def);
    let impl_block = generate_impl_block(def);
    let wrapper = generate_wrapper_struct(def, &deco);
    let tonic_trait_impl = trait_impl::generate_tonic_trait_impl(def, &deco);
    let grpc_service_impl = service_impl::generate_grpc_service_impl(def, &deco);
    let endpoint_deps_impl = service_impl::generate_endpoint_deps_impl(def);
    let deco_items = &deco.items;

    quote! {
        #impl_block
        #deco_items
        #wrapper
        #tonic_trait_impl
        #grpc_service_impl
        #endpoint_deps_impl
    }
}

/// Generate the user's impl block with route attributes stripped.
fn generate_impl_block(def: &GrpcRoutesImplDef) -> TokenStream {
    let controller_name = &def.controller_name;

    let methods: Vec<&syn::ImplItemFn> = def
        .methods
        .iter()
        .map(|m| &m.fn_item)
        .chain(def.other_methods.iter())
        .collect();

    quote! {
        impl #controller_name {
            #(#methods)*
        }
    }
}

/// Generate the wrapper struct that holds the controller core + the prebuilt
/// interceptor-set container, plus the container struct itself.
///
/// The wrapper is what actually implements the tonic trait. The controller
/// core is built ONCE from the bean graph (`ContextConstruct`) when the
/// service is registered; requests share it through the `Arc`. Interceptor
/// sets are built at the same time, from the same context
/// (`DecoratorSpec::build`) — never per call.
fn generate_wrapper_struct(def: &GrpcRoutesImplDef, deco: &GrpcDecoSets) -> TokenStream {
    let controller_name = &def.controller_name;
    let wrapper_name = quote::format_ident!("__R2eGrpc{}", controller_name);

    let (container_decl, decos_field) = if deco.has_any() {
        let container = GrpcDecoSets::container_ident(controller_name);
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
