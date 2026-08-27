//! Generate the hidden per-tool invocation methods on the wrapper struct.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;

use crate::codegen::decorators::wrap_with_deco_interceptors;
use crate::parsing::mcp_routes_parsing::{McpRoutesImplDef, McpTool, McpToolArg};
use crate::util::crate_path::{r2e_core_path, r2e_mcp_path};

use super::{invoke_ident, McpDecoSets};

/// Generate `impl __R2eMcp<Name> { async fn __r2e_tool_<fn>(...) ... }` — one
/// invocation method per tool.
pub fn generate_invoke_impl(def: &McpRoutesImplDef, deco: &McpDecoSets) -> TokenStream {
    if def.tools.is_empty() {
        return quote! {};
    }
    let krate = r2e_core_path();
    let mcp = r2e_mcp_path();
    let wrapper_name = super::wrapper_ident(&def.controller_name);

    let methods: Vec<TokenStream> = def
        .tools
        .iter()
        .enumerate()
        .map(|(i, tool)| generate_invoke_method(def, tool, deco.set_for(i), &krate, &mcp))
        .collect();

    quote! {
        #[doc(hidden)]
        impl #wrapper_name {
            #(#methods)*
        }
    }
}

/// One tool's invocation method. Body order:
/// scope check (`check_tool`, only when the tool declares scopes) →
/// identity extraction (required identity absent → `Unauthorized`, before
/// guards so `#[roles]` never sees a half-authenticated call) → guard checks
/// → params deserialization → method call (interceptor-wrapped, arguments in
/// original positional order) → `IntoToolResult`.
fn generate_invoke_method(
    def: &McpRoutesImplDef,
    tool: &McpTool,
    deco_set: Option<&crate::codegen::decorators::DecoSet>,
    krate: &TokenStream,
    mcp: &TokenStream,
) -> TokenStream {
    let fn_name = &tool.name;
    let invoke_name = invoke_ident(fn_name);
    let controller_name_str = def.controller_name.to_string();
    let fn_name_str = fn_name.to_string();
    let tool_name_str = tool.tool_name();

    // --- scope requirements prologue ---------------------------------------
    // Emitted only when the tool declares scopes: `check_tool` runs BEFORE
    // identity extraction and guards (a caller without the scope gets the
    // scope denial, not an identity/role error). Role requirements are
    // enforced by the guard below; unrestricted tools pay nothing.
    let scope_check = if !tool.meta.scopes.is_empty() || !tool.meta.any_scopes.is_empty() {
        let req = super::requirements_expr(def, tool, mcp);
        quote! {
            #mcp::__macro_support::check_tool(
                __call.parts.as_deref().map(|__p| &__p.extensions),
                #tool_name_str,
                &#req,
            )?;
        }
    } else {
        quote! {}
    };

    // --- identity extraction ---------------------------------------------
    let identity_param = tool.identity_param();
    let identity_stmts = match identity_param {
        Some(p) => {
            let id_ty = &p.ty;
            if p.is_optional {
                quote! {
                    let __identity: ::core::option::Option<#id_ty> =
                        __call.extension::<#id_ty>();
                }
            } else {
                quote! {
                    let __identity: #id_ty = match __call.extension::<#id_ty>() {
                        ::core::option::Option::Some(__v) => __v,
                        ::core::option::Option::None => {
                            return ::core::result::Result::Err(
                                #mcp::__macro_support::McpError::Unauthorized(
                                    ::std::format!(
                                        "tool `{}` requires an authenticated caller",
                                        #tool_name_str
                                    ),
                                ),
                            );
                        }
                    };
                }
            }
        }
        None => quote! {},
    };

    // --- guard checks ------------------------------------------------------
    let has_guards = deco_set.is_some_and(|s| !s.guard_fields.is_empty());
    let guard_stmts = if has_guards {
        let set = deco_set.unwrap();
        let deco_field = McpDecoSets::field_ident(fn_name);
        let identity_ref = match identity_param {
            Some(p) if p.is_optional => quote! { __identity.as_ref() },
            Some(_) => quote! { ::core::option::Option::Some(&__identity) },
            None => quote! {
                ::core::option::Option::<&#mcp::__macro_support::NoIdentity>::None
            },
        };
        let checks: Vec<TokenStream> = set
            .guard_fields
            .iter()
            .map(|field| {
                quote! {
                    if let ::core::result::Result::Err(__resp) =
                        #krate::Guard::check(&__gdeco.#field, &__gctx).await
                    {
                        return ::core::result::Result::Err(
                            #mcp::__macro_support::guard_response_to_error(__resp).await,
                        );
                    }
                }
            })
            .collect();
        quote! {
            {
                let __gdeco = &self.__decos.#deco_field;
                let __gctx = #mcp::__macro_support::tool_guard_context(
                    &__call,
                    #fn_name_str,
                    #controller_name_str,
                    #identity_ref,
                );
                #(#checks)*
            }
        }
    } else {
        quote! {}
    };

    // --- params deserialization ---------------------------------------------
    let params_stmts = match tool.params_type() {
        Some(params_ty) => {
            // Spanned at the params type so a missing `Deserialize`/`JsonSchema`
            // is a trait-bound error pointing at the user's type.
            quote_spanned! {params_ty.span()=>
                let __params = <#params_ty as #mcp::__macro_support::ToolParams>::from_arguments(
                    __call.arguments.clone(),
                )?;
            }
        }
        None => quote! {},
    };

    // --- method call (original positional argument order) -------------------
    let call_args: Vec<TokenStream> = tool
        .args
        .iter()
        .map(|arg| match arg {
            McpToolArg::Identity(_) => quote! { __identity },
            McpToolArg::Params(_) => quote! { #mcp::__macro_support::Params(__params) },
            McpToolArg::Call => quote! { __call.clone() },
            McpToolArg::Cancel => quote! { __call.cancel.clone() },
        })
        .collect();

    let method_call = quote! { __core.#fn_name(#(#call_args),*).await };

    // Interceptors are prebuilt wrapper fields (one set per tool, built once
    // from the bean graph in `tools()`); when the tool has none (or spec
    // inference failed — the `compile_error!` is already emitted) the call is
    // unwrapped.
    let has_intercepts = deco_set.is_some_and(|s| !s.intercept_fields.is_empty());
    let body = if has_intercepts {
        let set = deco_set.unwrap();
        let deco_field = McpDecoSets::field_ident(fn_name);
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
    } else {
        method_call
    };

    quote! {
        #[allow(non_snake_case)]
        async fn #invoke_name(
            &self,
            __call: #mcp::__macro_support::ToolCall,
        ) -> ::core::result::Result<
            #mcp::__macro_support::CallToolResult,
            #mcp::__macro_support::McpError,
        > {
            #scope_check
            #identity_stmts
            #guard_stmts
            #params_stmts
            let __core = ::std::sync::Arc::clone(&self.core);
            let __result = { #body };
            #mcp::__macro_support::IntoToolResult::into_tool_result(__result)
        }
    }
}
