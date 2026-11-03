//! The registration emitter shared by every endpoint kind: `#[routes]`
//! (`Controller<S, W>`, `codegen/controller_impl.rs`), `#[mcp_routes]`
//! (`McpService<S, W>`, `mcp_codegen/service_impl.rs`) and `#[grpc_routes]`
//! (`GrpcService`, `grpc_codegen/service_impl.rs`).
//!
//! Two pieces:
//!
//! - [`DepsFold`] — the `EndpointDeps::Deps` list: the core's
//!   `ContextConstruct::Deps` extended with every decorator site's
//!   `<Spec as DecoratorSpec>::Deps` (deduplicated by spec type), plus
//!   transport extras appended through [`DepsFold::append`] (`ManagedDeps` for
//!   HTTP `#[managed]` params, `GrpcIdentity::Spec` deps for gRPC). Emitted
//!   once on the state-independent `EndpointDeps` carrier
//!   ([`DepsFold::carrier_impl`]), checked by `AllSatisfied` at
//!   `register_*()` and by `ModuleDepsSatisfied` at `register_module()`.
//! - [`Witness`] — the extraction-site markers of a state-generic impl:
//!   `__R2eMd` for the request-data struct, then one `__R2eMp<Fn>` per member
//!   declaring an `#[inject(identity)]` parameter, bound
//!   `<declared ty>: FromRequestPartsVia<__R2eS, __R2eMp<Fn>>`. They are folded
//!   into the witness tuple `(__R2eMd, __R2eMp<Fn>, ...)` (E0207 forbids
//!   leaving them unconstrained on the impl); a plain service infers `((),)`.

use proc_macro2::TokenStream;
use quote::quote;

use super::decorators::{controller_site_exprs, deps_fold_from_base};
use crate::parsing::routes_parsing::RoutesImplDef;
use crate::util::crate_path::r2e_core_path;

/// An endpoint's `EndpointDeps::Deps` type-list expression.
pub(crate) struct DepsFold {
    krate: TokenStream,
    deps: TokenStream,
}

impl DepsFold {
    /// The core's `ContextConstruct::Deps` extended with every decorator
    /// site's spec deps — transport-neutral, so every endpoint kind carries
    /// the same shape of dep list. All lists are concrete, so the `TAppend`
    /// projections normalize without extra bounds on the impl.
    pub(crate) fn endpoint<'a>(
        endpoint_name: &syn::Ident,
        site_exprs: impl IntoIterator<Item = &'a syn::Expr>,
    ) -> Self {
        let krate = r2e_core_path();
        let deps = deps_fold_from_base(
            quote! { <#endpoint_name as #krate::ContextConstruct>::Deps },
            site_exprs,
        );
        Self { krate, deps }
    }

    /// Append a dep list (`<deps as TAppend<list>>::Output`).
    pub(crate) fn append(&mut self, list: TokenStream) {
        let krate = &self.krate;
        let deps = &self.deps;
        self.deps = quote! {
            <#deps as #krate::type_list::TAppend<
                #list,
            >>::Output
        };
    }

    /// The state-independent `EndpointDeps` carrier of the full dep list —
    /// lets `register_module` check decorator deps in the NoState phase,
    /// where a transport trait's `Deps` is not yet nameable.
    pub(crate) fn carrier_impl(&self, endpoint_name: &syn::Ident) -> TokenStream {
        let krate = &self.krate;
        let deps = &self.deps;
        quote! {
            #[doc(hidden)]
            impl #krate::EndpointDeps for #endpoint_name {
                type Deps = #deps;
            }
        }
    }
}

/// The `#[routes]` dep fold: every decorator site of the block, plus the
/// `ManagedDeps` of every distinct `#[managed]` parameter type.
///
/// Managed resources resolve their collaborators dynamically inside `acquire`
/// (`state.bean::<T>()`), which cannot fail at compile time — folding
/// `<Ty as ManagedDeps>::Deps` in here is what turns "pool was never provided"
/// from a runtime 500 into a `register_controller` compile error.
pub(crate) fn controller_deps_fold(def: &RoutesImplDef) -> DepsFold {
    let mut fold = DepsFold::endpoint(&def.controller_name, controller_site_exprs(def));
    let krate = fold.krate.clone();
    let mut seen = std::collections::HashSet::new();
    for rm in &def.route_methods {
        for mp in &rm.managed_params {
            let ty = crate::util::type_utils::staticize_lifetimes(&mp.ty);
            if seen.insert(quote!(#ty).to_string()) {
                fold.append(quote! { <#ty as #krate::ManagedDeps>::Deps });
            }
        }
    }
    fold
}

/// The extraction-site markers of a state-generic endpoint impl.
pub(crate) struct Witness {
    krate: TokenStream,
    state: syn::Ident,
    data_marker: syn::Ident,
    markers: Vec<syn::Ident>,
    bounds: Vec<TokenStream>,
}

impl Witness {
    pub(crate) fn new() -> Self {
        Self {
            krate: r2e_core_path(),
            state: super::handlers::state_generic(),
            data_marker: super::handlers::data_marker(),
            markers: Vec::new(),
            bounds: Vec::new(),
        }
    }

    /// Record a member's parameter-level `#[inject(identity)]`, extracted as
    /// `declared_ty` (`I` or `Option<I>`).
    pub(crate) fn push_identity(&mut self, member: &syn::Ident, declared_ty: TokenStream) {
        let krate = &self.krate;
        let state = &self.state;
        let marker = super::handlers::identity_marker_for(member);
        self.bounds.push(quote! {
            #declared_ty: #krate::web::extract::FromRequestPartsVia<#state, #marker>
        });
        self.markers.push(marker);
    }

    /// The request-data marker (`__R2eMd`).
    pub(crate) fn data_marker(&self) -> &syn::Ident {
        &self.data_marker
    }

    /// The impl generics after the state: `__R2eMd, __R2eMp<Fn>, ...`.
    pub(crate) fn generics(&self) -> TokenStream {
        let md = &self.data_marker;
        let markers = &self.markers;
        quote! { #md, #(#markers),* }
    }

    /// The witness tuple: `(__R2eMd, __R2eMp<Fn>, ...)`.
    pub(crate) fn tuple(&self) -> TokenStream {
        let md = &self.data_marker;
        let markers = &self.markers;
        quote! { (#md, #(#markers,)*) }
    }

    /// `__R2eMd: Send + Sync + 'static,`.
    pub(crate) fn data_marker_pred(&self) -> TokenStream {
        let md = &self.data_marker;
        quote! { #md: ::core::marker::Send + ::core::marker::Sync + 'static, }
    }

    /// `__R2eMp<Fn>: Send + Sync + 'static,` per identity parameter.
    pub(crate) fn marker_preds(&self) -> TokenStream {
        let markers = &self.markers;
        quote! { #(#markers: ::core::marker::Send + ::core::marker::Sync + 'static,)* }
    }

    /// `<declared ty>: FromRequestPartsVia<__R2eS, __R2eMp<Fn>>,` per identity
    /// parameter.
    pub(crate) fn identity_bounds(&self) -> TokenStream {
        let bounds = &self.bounds;
        quote! { #(#bounds,)* }
    }
}
