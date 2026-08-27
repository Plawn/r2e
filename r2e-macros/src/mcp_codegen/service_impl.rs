//! Generate `impl McpService` and `impl EndpointDeps` for the controller.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;

use crate::parsing::mcp_routes_parsing::{McpRoutesImplDef, McpTool};
use crate::util::crate_path::{r2e_core_path, r2e_mcp_path};

use super::McpDecoSets;

/// Every decorator site expression of the impl block, in dep-fold order:
/// controller-level interceptors and guards (only meaningful when at least
/// one tool exists), then per-tool guards and interceptors.
fn site_exprs(def: &McpRoutesImplDef) -> Vec<&syn::Expr> {
    let mut exprs: Vec<&syn::Expr> = Vec::new();
    if !def.tools.is_empty() {
        exprs.extend(&def.controller_intercepts);
        exprs.extend(&def.controller_guards);
    }
    for t in &def.tools {
        exprs.extend(&t.decorators.guard_fns);
        exprs.extend(&t.decorators.intercept_fns);
    }
    exprs
}

/// Generate the `EndpointDeps` carrier for the service: the core's
/// `ContextConstruct::Deps` extended with every `#[guard]`/`#[roles]`/
/// `#[intercept]` site's spec deps — the same fold `#[routes]` emits for
/// HTTP controllers. Checked by `AllSatisfied` at `register_mcp_service()`,
/// so a missing bean is a compile error at the registration call site.
pub fn generate_endpoint_deps_impl(def: &McpRoutesImplDef) -> TokenStream {
    let krate = r2e_core_path();
    let controller_name = &def.controller_name;
    let deps_fold =
        crate::codegen::decorators::endpoint_deps_fold(controller_name, site_exprs(def));

    quote! {
        #[doc(hidden)]
        impl #krate::EndpointDeps for #controller_name {
            type Deps = #deps_fold;
        }
    }
}

/// Generate `impl McpService for ControllerName`.
pub fn generate_mcp_service_impl(def: &McpRoutesImplDef, deco: &McpDecoSets) -> TokenStream {
    let krate = r2e_core_path();
    let mcp = r2e_mcp_path();
    let controller_name = &def.controller_name;
    let controller_name_str = controller_name.to_string();
    let wrapper_name = super::wrapper_ident(controller_name);

    // Prebuild every tool's guard/interceptor set from the resolved graph —
    // once, at registration, exactly like route decorator sets — into the
    // single Arc'd container.
    let decos_init = if deco.has_any() {
        let container = McpDecoSets::container_ident(controller_name);
        let field_inits: Vec<TokenStream> = deco
            .fields(def)
            .map(|(field, set)| {
                let ctor = &set.ctor_ident;
                quote! { #field: #ctor(__ctx) }
            })
            .collect();
        quote! {
            __decos: ::std::sync::Arc::new(#container {
                #(#field_inits,)*
            }),
        }
    } else {
        quote! {}
    };

    // Aggregated config validation: the core's own `#[config]`/
    // `#[config_section]` keys (from the `#[controller]`-generated meta
    // module) plus every decorator spec's declared keys. Reported at
    // `register_mcp_service()`, the MCP peer of `register_controller()`.
    let meta_mod = format_ident!("__r2e_meta_{}", controller_name);
    let decorator_config_stmts =
        crate::codegen::decorators::decorator_config_key_stmts(site_exprs(def));

    let tool_count = def.tools.len();
    let tool_pushes: Vec<TokenStream> = def
        .tools
        .iter()
        .map(|tool| generate_tool_route(tool, &mcp))
        .collect();

    quote! {
        impl #mcp::__macro_support::McpService for #controller_name {
            fn service_name() -> &'static str {
                #controller_name_str
            }

            fn validate_config(
                __config: &#krate::config::R2eConfig,
            ) -> ::std::vec::Vec<#krate::config::MissingKeyError> {
                #[allow(unused_mut)]
                let mut __errors = #meta_mod::validate_config(__config);
                #(#decorator_config_stmts)*
                __errors
            }

            fn tools(
                __ctx: &::std::sync::Arc<#krate::beans::BeanContext>,
            ) -> ::std::vec::Vec<#mcp::__macro_support::ToolRoute> {
                let __wrapper = #wrapper_name {
                    core: ::std::sync::Arc::new(
                        <#controller_name as #krate::ContextConstruct>::from_context(__ctx),
                    ),
                    #decos_init
                };
                let mut __tools = ::std::vec::Vec::with_capacity(#tool_count);
                #(#tool_pushes)*
                __tools
            }
        }
    }
}

/// One `__tools.push(ToolRoute { ... })` statement for a tool.
fn generate_tool_route(tool: &McpTool, mcp: &TokenStream) -> TokenStream {
    let invoke_name = super::invoke_ident(&tool.name);
    let tool_name_str = tool.tool_name();

    let title = opt_string(&tool.meta.title);
    let description = opt_string(&tool_description(tool));

    // Input schema: from the Params<T> inner type (spanned there so a
    // missing `JsonSchema` derive is a trait-bound error at the right spot);
    // parameterless tools advertise an empty object schema.
    let input_schema = match tool.params_type() {
        Some(params_ty) => quote_spanned! {params_ty.span()=>
            ::std::sync::Arc::new(
                <#params_ty as #mcp::__macro_support::ToolParams>::input_schema(),
            )
        },
        None => quote! {
            ::std::sync::Arc::new(#mcp::__macro_support::empty_object_schema())
        },
    };

    // Output schema: autoref-specialization probe over the JSON body type of
    // the return (`Result<Json<T>, _>` / `Json<T>` → `T`); types without
    // `JsonSchema` (or non-Json returns) advertise none.
    let output_schema = match output_body_type(&tool.fn_item.sig.output) {
        Some(out_ty) => output_schema_probe(&out_ty, mcp),
        None => quote! { ::core::option::Option::None },
    };

    let read_only = opt_bool(tool.meta.read_only);
    let destructive = opt_bool(tool.meta.destructive);
    let idempotent = opt_bool(tool.meta.idempotent);
    let open_world = opt_bool(tool.meta.open_world);

    quote! {
        {
            let __w = __wrapper.clone();
            __tools.push(#mcp::__macro_support::ToolRoute {
                name: #tool_name_str.into(),
                title: #title,
                description: #description,
                input_schema: #input_schema,
                output_schema: #output_schema,
                annotations: #mcp::__macro_support::ToolAnnotations {
                    title: ::core::option::Option::None,
                    read_only: #read_only,
                    destructive: #destructive,
                    idempotent: #idempotent,
                    open_world: #open_world,
                },
                invoke: ::std::sync::Arc::new(
                    move |__call: #mcp::__macro_support::ToolCall|
                        -> #mcp::__macro_support::ToolFuture {
                        let __w = __w.clone();
                        ::std::boxed::Box::pin(async move {
                            __w.#invoke_name(__call).await
                        })
                    },
                ),
            });
        }
    }
}

/// The tool description: explicit `#[tool(description = ...)]` override, or
/// the full doc comment (summary + body).
fn tool_description(tool: &McpTool) -> Option<String> {
    if tool.meta.description.is_some() {
        return tool.meta.description.clone();
    }
    tool.doc_text.clone()
}

fn opt_string(value: &Option<String>) -> TokenStream {
    match value {
        Some(s) => quote! { ::core::option::Option::Some(::std::string::String::from(#s)) },
        None => quote! { ::core::option::Option::None },
    }
}

fn opt_bool(value: Option<bool>) -> TokenStream {
    match value {
        Some(b) => quote! { ::core::option::Option::Some(#b) },
        None => quote! { ::core::option::Option::None },
    }
}

/// Unwrap `Result<T, E>` / `ApiResult<T>` / `JsonResult<T>` → `T`, leaving
/// other types unchanged. (Local copy of the private helper in
/// `codegen::controller_impl`.)
fn unwrap_result_type(ty: &syn::Type) -> &syn::Type {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let ident_str = segment.ident.to_string();
            if ident_str == "Result" || ident_str == "ApiResult" || ident_str == "JsonResult" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return inner;
                    }
                }
            }
        }
    }
    ty
}

/// Extract the inner type from `Json<T>` → `T`.
fn unwrap_json_type(ty: &syn::Type) -> Option<&syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Json" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// The JSON body type of a tool's return type, if any:
/// `Result<Json<T>, E>` / `Json<T>` → `T`.
fn output_body_type(output: &syn::ReturnType) -> Option<syn::Type> {
    let syn::ReturnType::Type(_, ty) = output else {
        return None;
    };
    unwrap_json_type(unwrap_result_type(ty)).cloned()
}

/// Autoref-specialization probe: `Some(schema)` when the type implements
/// `JsonSchema`, `None` otherwise — without requiring the bound.
fn output_schema_probe(ty: &syn::Type, mcp: &TokenStream) -> TokenStream {
    quote! {
        {
            struct __SchemaProbe<T>(::core::marker::PhantomData<T>);
            trait __NoSchema {
                fn __schema(
                    &self,
                ) -> ::core::option::Option<#mcp::__macro_support::SchemaObject> {
                    ::core::option::Option::None
                }
            }
            impl<T> __NoSchema for &__SchemaProbe<T> {}
            impl<T: #mcp::schemars::JsonSchema> __SchemaProbe<T> {
                fn __schema(
                    &self,
                ) -> ::core::option::Option<#mcp::__macro_support::SchemaObject> {
                    ::core::option::Option::Some(
                        #mcp::__macro_support::schema_object_for::<T>(),
                    )
                }
            }
            let __p = __SchemaProbe::<#ty>(::core::marker::PhantomData);
            #[allow(unused_imports)]
            use __NoSchema as _;
            (&__p).__schema().map(::std::sync::Arc::new)
        }
    }
}
