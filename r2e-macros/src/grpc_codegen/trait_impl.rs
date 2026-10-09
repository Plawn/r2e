//! Generate the tonic trait implementation for the gRPC wrapper struct.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::codegen::decorators::wrap_with_deco_interceptors;
use crate::parsing::grpc_routes_parsing::{GrpcMethod, GrpcRoutesImplDef};
use crate::util::crate_path::{r2e_core_path, r2e_grpc_path};

use super::GrpcDecoSets;

/// Generate `#[tonic::async_trait] impl TraitPath for __R2eGrpc<Name>`.
pub fn generate_tonic_trait_impl(def: &GrpcRoutesImplDef, deco: &GrpcDecoSets) -> TokenStream {
    let krate = r2e_core_path();
    let grpc_krate = r2e_grpc_path();
    let controller_name = &def.controller_name;
    let service_trait = &def.service_trait;
    let wrapper_name = format_ident!("__R2eGrpc{}", controller_name);

    let method_impls: Vec<TokenStream> = def
        .methods
        .iter()
        .enumerate()
        .map(|(i, m)| {
            generate_method_impl(m, deco.set_for(i), &krate, &grpc_krate, controller_name)
        })
        .collect();

    quote! {
        #[#grpc_krate::tonic::async_trait]
        impl #service_trait for #wrapper_name {
            #(#method_impls)*
        }
    }
}

/// Generate a single tonic trait method implementation.
///
/// Shape: identity extraction (if the method injects one) → guard checks
/// (controller sites then method sites, one shared `GuardContext` built from
/// the request) → interceptor chain → user method. Identity and guard
/// rejections return early as a `tonic::Status` (`rejection_to_status`).
fn generate_method_impl(
    method: &GrpcMethod,
    deco_set: Option<&crate::codegen::decorators::DecoSet>,
    krate: &TokenStream,
    grpc_krate: &TokenStream,
    controller_name: &syn::Ident,
) -> TokenStream {
    let fn_name = &method.name;
    let fn_item = &method.fn_item;
    let sig = &fn_item.sig;
    let identity_index = method.identity_param.as_ref().map(|p| p.index);

    // The request message parameter: the first typed param that is not the
    // identity parameter.
    let request_param = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => Some(pt),
            _ => None,
        })
        .enumerate()
        .find(|(idx, _)| Some(*idx) != identity_index)
        .map(|(_, pt)| pt);

    let return_type = &sig.output;

    let controller_name_str = controller_name.to_string();
    let fn_name_str = fn_name.to_string();

    // Build the request param for the tonic trait signature
    let request_param_tokens = if let Some(pt) = request_param {
        let ty = &pt.ty;
        quote! { request: #ty }
    } else {
        quote! { request: #grpc_krate::tonic::Request<()> }
    };

    // --- identity extraction ---------------------------------------------
    // The extractor (e.g. the `Arc<JwtClaimsValidator>` bean) was built once
    // at registration into the container; per call only the metadata lookup
    // + validation run. A required identity failing is `Unauthenticated`
    // before any guard sees the call.
    let identity_stmts = match &method.identity_param {
        Some(p) => {
            let id_ty = &p.ty;
            let id_field = GrpcDecoSets::identity_field_ident(fn_name);
            let (binding_ty, extract_fn) = if p.is_optional {
                (
                    quote! { ::core::option::Option<#id_ty> },
                    quote! { extract_optional },
                )
            } else {
                (quote! { #id_ty }, quote! { extract })
            };
            quote! {
                let __identity: #binding_ty = match
                    <#id_ty as #grpc_krate::__macro_support::GrpcIdentity>::#extract_fn(
                        &self.__decos.#id_field,
                        request.metadata(),
                    )
                    .await
                {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__rej) => {
                        return ::core::result::Result::Err(
                            #grpc_krate::__macro_support::rejection_to_status(__rej),
                        );
                    }
                };
            }
        }
        None => quote! {},
    };

    // --- guard checks ------------------------------------------------------
    let guard_stmts = match deco_set {
        Some(set) if !set.guard_fields.is_empty() => {
            let deco_field = GrpcDecoSets::field_ident(fn_name);
            let identity_ref = match &method.identity_param {
                Some(p) if p.is_optional => quote! { __identity.as_ref() },
                Some(_) => quote! { ::core::option::Option::Some(&__identity) },
                None => quote! { ::core::option::Option::<&#krate::NoIdentity>::None },
            };
            let checks: Vec<TokenStream> = set
                .guard_fields
                .iter()
                .map(|field| {
                    quote! {
                        if let ::core::result::Result::Err(__rej) =
                            #krate::Guard::check(&self.__decos.#deco_field.#field, &__gctx).await
                        {
                            return ::core::result::Result::Err(
                                #grpc_krate::__macro_support::rejection_to_status(__rej),
                            );
                        }
                    }
                })
                .collect();
            // Scoped: the context borrows `request`, which the user method
            // then takes by value.
            quote! {
                {
                    let __gctx = #grpc_krate::__macro_support::guard_context(
                        &request,
                        #fn_name_str,
                        #controller_name_str,
                        #identity_ref,
                    );
                    #(#checks)*
                }
            }
        }
        _ => quote! {},
    };

    // The core is shared — built once at registration, cloned per call site.
    let construct_controller = quote! {
        let __ctrl = ::std::sync::Arc::clone(&self.core);
    };

    // Build the method call: the identity parameter receives the extracted
    // identity, the message parameter the request.
    let call_args: Vec<TokenStream> = sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(_) => Some(()),
            _ => None,
        })
        .enumerate()
        .map(|(idx, ())| {
            if Some(idx) == identity_index {
                quote! { __identity }
            } else {
                quote! { request }
            }
        })
        .collect();

    let method_call = quote! { __ctrl.#fn_name(#(#call_args),*).await };

    // Interceptors are prebuilt wrapper fields (one set per method, built
    // once from the bean graph in `add_to_routes`); `deco_set` is `None` when
    // the method has no decorator sites or when spec inference failed (the
    // `compile_error!` is already emitted — degrade to the unwrapped shape).
    let body = match deco_set {
        Some(set) if !set.intercept_fields.is_empty() => {
            let deco_field = GrpcDecoSets::field_ident(fn_name);
            let wrapped = wrap_with_deco_interceptors(
                method_call,
                &fn_name_str,
                &controller_name_str,
                &set.intercept_fields,
                krate,
            );
            quote! {
                let __deco = &self.__decos.#deco_field;
                #wrapped
            }
        }
        _ => method_call,
    };

    quote! {
        async fn #fn_name(&self, #request_param_tokens) #return_type {
            #identity_stmts
            #guard_stmts
            #construct_controller
            #body
        }
    }
}
