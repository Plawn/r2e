//! Generate the tonic trait implementation for the gRPC wrapper struct.

use proc_macro2::TokenStream;
use quote::quote;

use crate::codegen::decorators::{wrap_with_interceptor_refs, MemberDecoSets};
use crate::codegen::handlers::{identity_marker_for, qualified_controller_name};
use crate::codegen::identity_path::{self, Arrival, FacadeNames, GuardIdentity};
use crate::parsing::grpc_routes_parsing::{GrpcMethod, GrpcRoutesImplDef};
use crate::util::crate_path::{r2e_core_path, r2e_grpc_path};

use super::{
    controller_items, identity_declared_ty, service_bounds, service_name_expr, witness,
    ControllerItems, GrpcDecoSets,
};

/// Generate `#[tonic::async_trait] impl TraitPath for __R2eGrpc<Name><S, W>`.
pub fn generate_tonic_trait_impl(def: &GrpcRoutesImplDef, deco: &GrpcDecoSets) -> TokenStream {
    let krate = r2e_core_path();
    let grpc_krate = r2e_grpc_path();
    let controller_name = &def.controller_name;
    let service_trait = &def.service_trait;
    let items = controller_items(controller_name);
    let wrapper_name = &items.wrapper_name;
    let service_name = service_name_expr(service_trait);

    let witness = witness(def);
    let witness_generics = witness.generics();
    let witness_tuple = witness.tuple();
    let bounds = service_bounds(def, &witness);
    let assoc_types = &def.assoc_types;

    let method_impls: Vec<TokenStream> = def
        .methods
        .iter()
        .enumerate()
        .map(|(i, m)| {
            generate_method_impl(
                m,
                &deco.sets,
                i,
                &krate,
                &grpc_krate,
                controller_name,
                &items,
                &service_name,
            )
        })
        .collect();

    quote! {
        #[#grpc_krate::tonic::async_trait]
        impl<__R2eS, #witness_generics> #service_trait
            for #wrapper_name<__R2eS, #witness_tuple>
        where
            #bounds
        {
            #(#assoc_types)*
            #(#method_impls)*
        }
    }
}

/// The proto method name of a Rust trait method (`say_hello` → `SayHello`):
/// the request-URI fallback when the call did not come through the
/// `CaptureMethodPath` layer (a test driving the trait directly).
fn proto_method_name(fn_name: &str) -> String {
    fn_name
        .split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

/// Generate a single tonic trait method implementation.
///
/// Shape: HTTP head from the tonic request → struct request scope + façade
/// → identity parameter (if the method injects one) → guard checks →
/// tonic request rebuilt → interceptor chain → user method on the façade.
/// Extraction and guard rejections return early as a `tonic::Status`
/// (`rejection_to_status`). Everything runs before the user method, so for a
/// server-streaming rpc the head is fully extracted before the stream opens.
///
/// Guard scoping mirrors HTTP: the shared controller-level guards are checked
/// against a controller-scoped `GuardContext` (`method_name = "*"`), then the
/// method's own guards against a method-scoped one (`method_name = <fn>`).
/// A controller-site guard is never checked against the method's own name,
/// so a stateful impl-level guard (`RateLimit`) keeps one service-wide bucket
/// and never shares a bucket with a method-level site.
#[allow(clippy::too_many_arguments)]
fn generate_method_impl(
    method: &GrpcMethod,
    sets: &MemberDecoSets,
    index: usize,
    krate: &TokenStream,
    grpc_krate: &TokenStream,
    controller_name: &syn::Ident,
    items: &ControllerItems,
    service_name: &TokenStream,
) -> TokenStream {
    let fn_name = &method.member.name;
    let fn_item = &method.member.fn_item;
    let sig = &fn_item.sig;
    let identity_index = method.member.identity_param.as_ref().map(|p| p.index);
    let meta_mod = &items.meta_mod;

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
    let proto_method = proto_method_name(&fn_name_str);

    // Build the request param for the tonic trait signature
    let request_param_tokens = if let Some(pt) = request_param {
        let ty = &pt.ty;
        quote! { request: #ty }
    } else {
        quote! { request: #grpc_krate::tonic::Request<()> }
    };

    // --- head --------------------------------------------------------------
    // The metadata map is deposited in the extensions only when something may
    // read it (the struct request scope or an identity parameter): it costs
    // one header-map clone.
    let has_identity_param = method.member.identity_param.is_some();
    let head = quote! {
        let (mut __parts, __msg) = #grpc_krate::__macro_support::into_head(
            request,
            || #grpc_krate::__macro_support::fallback_method_path(#service_name, #proto_method),
            #meta_mod::HAS_REQUEST_SCOPE || #has_identity_param,
        );
    };

    // --- struct request scope + façade --------------------------------------
    let reject = quote! {
        return ::core::result::Result::Err(
            #grpc_krate::__macro_support::rejection_to_status(__rej),
        )
    };
    let bind = identity_path::bind_facade(
        &FacadeNames {
            meta_mod,
            data_name: &items.data_name,
            data_marker: &items.data_marker,
        },
        quote! { ::std::sync::Arc::clone(&self.core) },
        Arrival::ExtractHere {
            krate,
            state: quote! { &*self.state },
            on_reject: reject,
        },
    );

    // --- identity parameter ------------------------------------------------
    // The same contract as an HTTP route's identity parameter: extracted
    // through `FromRequestPartsVia<S, M>` against the state (the validator is
    // a bean). A required identity failing is `Unauthenticated` before any
    // guard sees the call.
    let identity_stmts = match identity_declared_ty(method) {
        Some(declared_ty) => {
            let marker = identity_marker_for(fn_name);
            quote! {
                let __identity: #declared_ty = match <#declared_ty as
                    #krate::web::extract::FromRequestPartsVia<__R2eS, #marker>>
                    ::from_request_parts_via(&mut __parts, &*self.state)
                    .await
                {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return ::core::result::Result::Err(
                            #grpc_krate::__macro_support::rejection_to_status(
                                ::core::convert::Into::<#krate::Rejection>::into(__e),
                            ),
                        );
                    }
                };
            }
        }
        None => quote! {},
    };

    // --- guard checks ------------------------------------------------------
    let decos = quote! { self.__decos };
    let ctrl_guards = sets.ctrl_guards(&decos);
    let method_guards = sets.member_guards(index, &decos);
    let guard_stmts = if ctrl_guards.is_empty() && method_guards.is_empty() {
        quote! {}
    } else {
        let binding = syn::Ident::new("__identity", proc_macro2::Span::call_site());
        let identity_ref = identity_path::guard_identity(
            meta_mod,
            match &method.member.identity_param {
                Some(p) => GuardIdentity::Param {
                    binding: &binding,
                    optional: p.is_optional,
                },
                None => GuardIdentity::Facade,
            },
        );
        // Module-qualified, like HTTP: identically named services in two
        // modules must not share identity-scoped guard state.
        let controller_name_q = qualified_controller_name(controller_name);
        let checks = |places: &[TokenStream], method_name: TokenStream| {
            if places.is_empty() {
                return quote! {};
            }
            let checks = places.iter().map(|place| {
                quote! {
                    if let ::core::result::Result::Err(__rej) =
                        #krate::Guard::check(&#place, &__gctx).await
                    {
                        return ::core::result::Result::Err(
                            #grpc_krate::__macro_support::rejection_to_status(__rej),
                        );
                    }
                }
            });
            // Scoped: the context borrows the head, which is then moved
            // back into the tonic request the user method takes by value.
            quote! {
                {
                    let __gctx = #grpc_krate::__macro_support::GuardContext::from_parts(
                        &__parts,
                        #method_name,
                        #controller_name_q,
                        #identity_ref,
                        #grpc_krate::__macro_support::PathParams::EMPTY,
                    );
                    #(#checks)*
                }
            }
        };
        let ctrl = checks(&ctrl_guards, quote! { "*" });
        let own = checks(&method_guards, quote! { #fn_name_str });
        quote! { #ctrl #own }
    };

    // The tonic request the user method receives: the head (with whatever
    // extractors left in its extensions) back around the message.
    let rebuild = quote! {
        let request = #grpc_krate::__macro_support::from_head(__parts, __msg);
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

    // Interceptors are prebuilt container fields: the shared controller-level
    // ones (outermost) then the method's own. A set is `None` when it has no
    // decorator sites or when spec inference failed (the `compile_error!` is
    // already emitted — degrade to the unwrapped shape).
    let interceptor_refs: Vec<TokenStream> = sets
        .intercepts(index, &quote! { __decos })
        .into_iter()
        .map(|place| quote! { &#place })
        .collect();
    let body = if interceptor_refs.is_empty() {
        method_call
    } else {
        let wrapped = wrap_with_interceptor_refs(
            method_call,
            &fn_name_str,
            &controller_name_str,
            &interceptor_refs,
            krate,
        );
        quote! {
            let __decos = &*self.__decos;
            #wrapped
        }
    };

    quote! {
        async fn #fn_name(&self, #request_param_tokens) #return_type {
            #head
            #bind
            #identity_stmts
            #guard_stmts
            #rebuild
            #body
        }
    }
}
