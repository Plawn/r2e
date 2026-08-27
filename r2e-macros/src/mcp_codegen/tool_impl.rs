//! Generate the hidden per-member invocation methods on the wrapper struct.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;

use crate::codegen::decorators::wrap_with_interceptor_refs;
use crate::parsing::mcp_routes_parsing::{McpMemberKind, McpRoutesImplDef, McpTool, McpToolArg};
use crate::util::crate_path::{r2e_core_path, r2e_mcp_path};

use super::{invoke_ident, McpDecoLayout};

/// Generate `impl __R2eMcp<Name> { async fn __r2e_<kind>_<fn>(...) ... }` —
/// one invocation method per member.
pub fn generate_invoke_impl(def: &McpRoutesImplDef, deco: &McpDecoLayout) -> TokenStream {
    if def.members.is_empty() {
        return quote! {};
    }
    let krate = r2e_core_path();
    let mcp = r2e_mcp_path();
    let wrapper_name = super::wrapper_ident(&def.controller_name);

    let methods: Vec<TokenStream> = def
        .members
        .iter()
        .enumerate()
        .map(|(i, tool)| generate_invoke_method(def, tool, deco, i, &krate, &mcp))
        .collect();

    quote! {
        #[doc(hidden)]
        impl #wrapper_name {
            #(#methods)*
        }
    }
}

/// One member's invocation method. Body order:
/// scope check (`check_access`, only when the member declares scopes) →
/// identity extraction (required identity absent → `Unauthorized`, before
/// guards so `#[roles]` never sees a half-authenticated call) → guard checks
/// → params deserialization (tools/prompts) → method call
/// (interceptor-wrapped, arguments in original positional order) → the
/// family's result conversion (`IntoToolResult` / `IntoResourceResult` /
/// `IntoPromptResult`).
fn generate_invoke_method(
    def: &McpRoutesImplDef,
    tool: &McpTool,
    deco: &McpDecoLayout,
    member_index: usize,
    krate: &TokenStream,
    mcp: &TokenStream,
) -> TokenStream {
    let fn_name = &tool.name;
    let kind = tool.kind;
    let kind_str = kind.attr_name();
    let invoke_name = invoke_ident(kind, fn_name);
    let controller_name_str = def.controller_name.to_string();
    let fn_name_str = fn_name.to_string();
    let tool_name_str = tool.tool_name();

    // --- scope requirements prologue ---------------------------------------
    // Emitted only when the member declares scopes: `check_access` runs
    // BEFORE identity extraction and guards (a caller without the scope gets
    // the scope denial, not an identity/role error). Role requirements are
    // enforced by the guard below; unrestricted members pay nothing.
    let scope_check = if !tool.meta.scopes.is_empty() || !tool.meta.any_scopes.is_empty() {
        let req = super::requirements_expr(def, tool, mcp);
        quote! {
            #mcp::__macro_support::check_access(
                __call.parts.as_deref().map(|__p| &__p.extensions),
                #kind_str,
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
                                        "{} `{}` requires an authenticated caller",
                                        #kind_str,
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
    let member_deco = deco.member(member_index);
    let has_controller_guards = !deco.controller_guard_fields.is_empty();
    let has_member_guards = !member_deco.guard_fields.is_empty();
    let has_guards = has_controller_guards || has_member_guards;
    let guard_stmts = if has_guards {
        let identity_ref = match identity_param {
            Some(p) if p.is_optional => quote! { __identity.as_ref() },
            Some(_) => quote! { ::core::option::Option::Some(&__identity) },
            None => quote! {
                ::core::option::Option::<&#mcp::__macro_support::NoIdentity>::None
            },
        };
        let controller_checks: Vec<TokenStream> = deco
            .controller_guard_fields
            .iter()
            .map(|field| {
                quote! {
                    if let ::core::result::Result::Err(__resp) =
                        #krate::Guard::check(&self.#field, &__ctrl_gctx).await
                    {
                        return ::core::result::Result::Err(
                            #mcp::__macro_support::guard_response_to_error(__resp).await,
                        );
                    }
                }
            })
            .collect();
        let member_checks: Vec<TokenStream> = member_deco
            .guard_fields
            .iter()
            .map(|field| {
                quote! {
                    if let ::core::result::Result::Err(__resp) =
                        #krate::Guard::check(&self.#field, &__member_gctx).await
                    {
                        return ::core::result::Result::Err(
                            #mcp::__macro_support::guard_response_to_error(__resp).await,
                        );
                    }
                }
            })
            .collect();
        let controller_context = if has_controller_guards {
            quote! {
                let __ctrl_gctx = #mcp::__macro_support::member_guard_context(
                    __call.parts.as_deref(),
                    "*",
                    #controller_name_str,
                    #identity_ref,
                );
                #(#controller_checks)*
            }
        } else {
            quote! {}
        };
        let member_context = if has_member_guards {
            quote! {
                let __member_gctx = #mcp::__macro_support::member_guard_context(
                    __call.parts.as_deref(),
                    #fn_name_str,
                    #controller_name_str,
                    #identity_ref,
                );
                #(#member_checks)*
            }
        } else {
            quote! {}
        };
        quote! {
            {
                #controller_context
                #member_context
            }
        }
    } else {
        quote! {}
    };

    // --- params deserialization ---------------------------------------------
    let has_call = tool.args.iter().any(|arg| matches!(arg, McpToolArg::Call));
    let has_cancel = tool
        .args
        .iter()
        .any(|arg| matches!(arg, McpToolArg::Cancel));
    let params_stmts = match tool.params_type() {
        Some(params_ty) => {
            // Preserve the arguments inside a call passed to the method. This
            // is the only clone needed for the Params + Call combination.
            let arguments = if has_call {
                quote! { __call.arguments.clone() }
            } else {
                quote! { __call.arguments }
            };
            // Spanned at the params type so a missing `Deserialize`/`JsonSchema`
            // is a trait-bound error pointing at the user's type.
            quote_spanned! {params_ty.span()=>
                let __params = <#params_ty as #mcp::__macro_support::ToolParams>::from_arguments(
                    #arguments,
                )?;
            }
        }
        None => quote! {},
    };

    // A call is moved into the method. Save only fields that are also needed
    // independently, instead of cloning the complete call context.
    let saved_call_fields = {
        let cancel = (has_call && has_cancel).then(|| {
            quote! { let __cancel = __call.cancel.clone(); }
        });
        let resource_uri = (has_call && kind == McpMemberKind::Resource).then(|| {
            quote! { let __resource_uri = __call.uri.clone(); }
        });
        quote! {
            #cancel
            #resource_uri
        }
    };

    // --- method call (original positional argument order) -------------------
    let call_args: Vec<TokenStream> = tool
        .args
        .iter()
        .map(|arg| match arg {
            McpToolArg::Identity(_) => quote! { __identity },
            McpToolArg::Params(_) => quote! { #mcp::__macro_support::Params(__params) },
            McpToolArg::Call => quote! { __call },
            McpToolArg::Cancel if has_call => quote! { __cancel },
            McpToolArg::Cancel => quote! { __call.cancel.clone() },
        })
        .collect();

    let method_call = quote! { __core.#fn_name(#(#call_args),*).await };

    // Impl-level products live once on the wrapper and precede the member's
    // own products. References are borrowed for the duration of this invoke;
    // no nested Arc clone is needed.
    let interceptor_refs: Vec<TokenStream> = deco
        .controller_intercept_fields
        .iter()
        .chain(&member_deco.intercept_fields)
        .map(|field| quote! { &self.#field })
        .collect();
    let has_intercepts = !interceptor_refs.is_empty();
    let body = if has_intercepts {
        wrap_with_interceptor_refs(
            method_call,
            &fn_name_str,
            &controller_name_str,
            &interceptor_refs,
            krate,
        )
    } else {
        method_call
    };

    // --- family-specific call type, return type and result conversion ------
    let call_ty = match kind {
        McpMemberKind::Tool => quote! { #mcp::__macro_support::ToolCall },
        McpMemberKind::Resource => quote! { #mcp::__macro_support::ResourceCall },
        McpMemberKind::Prompt => quote! { #mcp::__macro_support::PromptCall },
    };
    let ok_ty = match kind {
        McpMemberKind::Tool => quote! { #mcp::__macro_support::CallToolResult },
        McpMemberKind::Resource => quote! {
            ::std::vec::Vec<#mcp::__macro_support::ResourceContents>
        },
        McpMemberKind::Prompt => quote! { #mcp::__macro_support::GetPromptResult },
    };
    let convert = match kind {
        McpMemberKind::Tool => quote! {
            #mcp::__macro_support::IntoToolResult::into_tool_result(__result)
        },
        McpMemberKind::Resource => {
            let mime = match &tool.meta.mime_type {
                Some(m) => quote! { ::core::option::Option::Some(#m) },
                None => quote! { ::core::option::Option::None },
            };
            let uri = if has_call {
                quote! { &__resource_uri }
            } else {
                quote! { &__call.uri }
            };
            quote! {
                #mcp::__macro_support::IntoResourceResult::into_resource_result(
                    __result,
                    #uri,
                    #mime,
                )
            }
        }
        McpMemberKind::Prompt => {
            let desc = match tool.meta.description.as_ref().or(tool.doc_text.as_ref()) {
                Some(d) => quote! { ::core::option::Option::Some(#d) },
                None => quote! { ::core::option::Option::None },
            };
            quote! {
                #mcp::__macro_support::IntoPromptResult::into_prompt_result(__result, #desc)
            }
        }
    };

    quote! {
        #[allow(non_snake_case)]
        async fn #invoke_name(
            &self,
            __call: #call_ty,
        ) -> ::core::result::Result<#ok_ty, #mcp::__macro_support::McpError> {
            #scope_check
            #identity_stmts
            #guard_stmts
            #params_stmts
            #saved_call_fields
            let __core = &self.core;
            let __result = { #body };
            #convert
        }
    }
}
