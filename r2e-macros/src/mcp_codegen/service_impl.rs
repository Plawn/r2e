//! Generate `impl McpService<S, W>` and `impl EndpointDeps` for the controller.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;

use crate::parsing::mcp_routes_parsing::{McpMemberKind, McpRoutesImplDef, McpTool};
use crate::util::crate_path::{r2e_core_path, r2e_mcp_path};
use crate::util::type_utils::{unwrap_json_type, unwrap_result_type};

use crate::codegen::decorators::MemberDecoSets;
use crate::codegen::registration::{DepsFold, Witness};

/// Every decorator site expression of the impl block, in dep-fold order:
/// controller-level interceptors and guards (only meaningful when at least
/// one member exists), then per-member guards and interceptors.
fn site_exprs(def: &McpRoutesImplDef) -> Vec<&syn::Expr> {
    let mut exprs: Vec<&syn::Expr> = Vec::new();
    if !def.members.is_empty() {
        exprs.extend(&def.controller_intercepts);
        exprs.extend(&def.controller_guards);
    }
    for t in &def.members {
        exprs.extend(&t.member.decorators.guard_fns);
        exprs.extend(&t.member.decorators.intercept_fns);
    }
    exprs
}

/// Generate the `EndpointDeps` carrier for the service: the core's
/// `ContextConstruct::Deps` extended with every `#[guard]`/`#[roles]`/
/// `#[intercept]` site's spec deps — the same fold `#[routes]` emits for
/// HTTP controllers. Checked by `AllSatisfied` at `register_mcp_service()`,
/// so a missing bean is a compile error at the registration call site.
pub fn generate_endpoint_deps_impl(def: &McpRoutesImplDef) -> TokenStream {
    DepsFold::endpoint(&def.controller_name, site_exprs(def)).carrier_impl(&def.controller_name)
}

/// Generate the state-generic `impl McpService<__R2eS, W> for ControllerName`.
///
/// Mirrors the `Controller<S, W>` impl `#[routes]` emits: the impl is generic
/// over the state `__R2eS`, the request-data marker `__R2eMd` (bound
/// `__R2eRequestData_<Name><__R2eMd>: RequestData<__R2eS>` — the struct's
/// `#[inject(identity)]` / `#[inject(request)]` fields, extracted once per
/// HTTP request) plus one opaque marker `__R2eMp<Fn>` per member that
/// declares an `#[inject(identity)]` parameter, bound
/// `<declared ty>: FromRequestPartsVia<__R2eS, __R2eMp<Fn>>`. The markers are
/// folded into the witness `W` (E0207 forbids leaving them unconstrained on
/// the impl), so `register_mcp_service::<Svc>()` infers them.
pub fn generate_mcp_service_impl(def: &McpRoutesImplDef, deco: &MemberDecoSets) -> TokenStream {
    let krate = r2e_core_path();
    let mcp = r2e_mcp_path();
    let controller_name = &def.controller_name;
    let controller_name_str = controller_name.to_string();
    let wrapper_name = super::wrapper_ident(controller_name);

    // Products are built once and stored directly beside the core in the
    // single Arc'd wrapper — no nested Arc/container and no per-member ctor.
    let deco_field_inits = deco.field_inits();

    // Aggregated config validation: the core's own `#[config]`/
    // `#[config_section]` keys (from the `#[controller]`-generated meta
    // module) plus every decorator spec's declared keys. Reported at
    // `register_mcp_service()`, the MCP peer of `register_controller()`.
    let super::ControllerItems {
        meta_mod,
        data_name,
        data_marker,
        ..
    } = super::controller_items(controller_name);
    let decorator_config_stmts =
        crate::codegen::decorators::decorator_config_key_stmts(site_exprs(def));

    let uses_session = def
        .uses_session
        .then(|| quote! { __routes.uses_session = true; });
    let mut witness = Witness::new();
    for member in &def.members {
        if let Some(site) = super::identity_site(member) {
            witness.push_identity(&member.member.name, site.declared_ty);
        }
    }
    let witness_generics = witness.generics();
    let witness_tuple = witness.tuple();
    let data_marker_pred = witness.data_marker_pred();
    let marker_preds = witness.marker_preds();
    let identity_bounds = witness.identity_bounds();
    // One shared copy of the state: the once-per-request extraction of the
    // struct's request scope runs against it, and so does every member that
    // extracts a parameter identity; each call clones the `Arc`, not the
    // state. Unused (and dropped) when the service has neither.
    let state_share = quote! {
        let __r2e_state = ::std::sync::Arc::new(::core::clone::Clone::clone(__state));
        let _ = &__r2e_state;
    };

    // The struct's request scope, extracted ONCE per HTTP request by the
    // endpoint layer — the HTTP handler's `RequestData::extract` call, run
    // before any JSON-RPC message of the request is dispatched, so a
    // required identity fails the whole request closed (401) exactly like
    // an HTTP route. The extracted data is deposited in the parts for the
    // members to bind their façade from; the subject (and roles, when the
    // identity type carries them) is reported back for session binding and
    // list filtering. Skipped entirely for a service without request scope.
    let request_scope = quote! {
        if #meta_mod::HAS_REQUEST_SCOPE {
            let __s = ::std::sync::Arc::clone(&__r2e_state);
            __routes.request_scope = ::core::option::Option::Some(
                #mcp::__macro_support::RequestScope::new(move |__parts| {
                    let __s = ::std::sync::Arc::clone(&__s);
                    ::std::boxed::Box::pin(async move {
                        let __data = <#data_name<#data_marker> as
                            #krate::web::extract::RequestData<__R2eS>>::extract(
                                __parts,
                                &*__s,
                            )
                            .await?;
                        let __caller = {
                            #[allow(unused_imports)]
                            use #mcp::__macro_support::{
                                CallerViaIdentity as _, CallerViaNone as _, CallerViaRoles as _,
                            };
                            (&&#mcp::__macro_support::CallerProbe::<#meta_mod::IdentityType>::new())
                                .caller(#meta_mod::request_identity(&__data))
                        };
                        #mcp::__macro_support::deposit_request_data(__parts, __data);
                        ::core::result::Result::Ok(__caller)
                    })
                }),
            );
        }
    };

    let member_pushes: Vec<TokenStream> = def
        .members
        .iter()
        .map(|member| match member.kind {
            McpMemberKind::Tool => generate_tool_route(def, member, &mcp),
            McpMemberKind::Resource => generate_resource_route(def, member, &mcp),
            McpMemberKind::Prompt => generate_prompt_route(def, member, &mcp),
            // Wired into the prompt/resource routes that reference it.
            McpMemberKind::Completion => quote! {},
        })
        .collect();

    quote! {
        impl<__R2eS, #witness_generics>
            #mcp::__macro_support::McpService<__R2eS, #witness_tuple>
            for #controller_name
        where
            __R2eS: ::core::clone::Clone
                + ::core::marker::Send
                + ::core::marker::Sync
                + 'static,
            #data_marker_pred
            #data_name<#data_marker>: #krate::web::extract::RequestData<__R2eS>
                + ::core::clone::Clone
                + ::core::marker::Send
                + ::core::marker::Sync
                + 'static,
            #marker_preds
            #identity_bounds
        {
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

            fn construct(
                _state: &__R2eS,
                __ctx: &#krate::beans::BeanContext,
            ) -> Self {
                <#controller_name as #krate::ContextConstruct>::from_context(__ctx)
            }

            fn routes(
                __state: &__R2eS,
                __core: ::std::sync::Arc<Self>,
                __ctx: &::std::sync::Arc<#krate::beans::BeanContext>,
            ) -> #mcp::__macro_support::McpRoutes {
                #state_share
                let __wrapper = ::std::sync::Arc::new(#wrapper_name {
                    core: __core,
                    #(#deco_field_inits,)*
                });
                let mut __routes = #mcp::__macro_support::McpRoutes::default();
                #uses_session
                #request_scope
                #(#member_pushes)*
                __routes
            }
        }
    }
}

/// The dispatch pieces of one member's route closure: what to capture next
/// to the wrapper clone (`outer`), what to clone per call (`inner`), and the
/// invoke call itself. A member with an identity parameter also captures the
/// shared state `Arc` and names its marker (not inferable from the call).
pub(super) struct InvokeCall {
    outer: TokenStream,
    inner: TokenStream,
    call: TokenStream,
}

pub(super) fn invoke_call(member: &McpTool) -> InvokeCall {
    let invoke_name = super::invoke_ident(member.kind, &member.member.name);
    // The request-data marker is not inferable from the call either: it is
    // the impl's own generic, named on every invoke.
    let md = crate::codegen::handlers::data_marker();
    match super::identity_site(member) {
        Some(site) => {
            let marker = &site.marker;
            InvokeCall {
                outer: quote! { let __s = ::std::sync::Arc::clone(&__r2e_state); },
                inner: quote! { let __s = ::std::sync::Arc::clone(&__s); },
                call: quote! {
                    __w.#invoke_name::<#md, __R2eS, #marker>(__call, &*__s).await
                },
            }
        }
        None => InvokeCall {
            outer: quote! {},
            inner: quote! {},
            call: quote! { __w.#invoke_name::<#md>(__call).await },
        },
    }
}

/// The `meta:` field initializer: `#[tool(meta = <expr>)]` converts the
/// expression through `Into<MetaObject>` (spanned at the expression so a
/// type mismatch points at the attribute), absent otherwise.
fn meta_expr(member: &McpTool) -> TokenStream {
    match &member.meta.meta {
        Some(expr) => quote_spanned! {expr.span()=>
            ::core::option::Option::Some(::core::convert::Into::into(#expr))
        },
        None => quote! { ::core::option::Option::None },
    }
}

/// One `__routes.tools.push(ToolRoute { ... })` statement for a tool.
fn generate_tool_route(def: &McpRoutesImplDef, tool: &McpTool, mcp: &TokenStream) -> TokenStream {
    let InvokeCall { outer, inner, call } = invoke_call(tool);
    let tool_name_str = tool.tool_name();

    let title = opt_string(&tool.meta.title);
    // Borrowed, not owned: the description travels into rmcp's
    // `Tool::description` (a `Cow<'static, str>`) and the whole `tools/list`
    // payload is cloned per request — a literal must not re-allocate there.
    let description = opt_cow_str(&tool_description(tool));

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
    let output_schema = match output_body_type(&tool.member.fn_item.sig.output) {
        Some(out_ty) => output_schema_probe(&out_ty, mcp),
        None => quote! { ::core::option::Option::None },
    };

    let group = group_expr(tool, mcp);
    let requirements = super::requirements_expr(def, tool, mcp);
    let read_only = opt_bool(tool.meta.read_only);
    let destructive = opt_bool(tool.meta.destructive);
    let idempotent = opt_bool(tool.meta.idempotent);
    let open_world = opt_bool(tool.meta.open_world);
    let meta = meta_expr(tool);

    quote! {
        {
            let __w = __wrapper.clone();
            #outer
            __routes.tools.push(#mcp::__macro_support::ToolRoute {
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
                requirements: #requirements,
                group: #group,
                meta: #meta,
                invoke: ::std::sync::Arc::new(
                    move |__call: #mcp::__macro_support::ToolCall|
                        -> #mcp::__macro_support::ToolFuture {
                        let __w = __w.clone();
                        #inner
                        ::std::boxed::Box::pin(async move { #call })
                    },
                ),
            });
        }
    }
}

/// One `__routes.resources.push(ResourceRoute { ... })` statement for a
/// resource. The URI is fixed (`#[resource(uri = "...")]`, validated at
/// parse time); no input schema — `resources/read` carries no arguments.
fn generate_resource_route(
    def: &McpRoutesImplDef,
    resource: &McpTool,
    mcp: &TokenStream,
) -> TokenStream {
    let InvokeCall { outer, inner, call } = invoke_call(resource);
    let uri = resource.meta.uri.as_deref().expect("parse() validated uri");
    let name_str = resource.tool_name();
    let title = opt_string(&resource.meta.title);
    let description = opt_string(&tool_description(resource));
    let mime_type = opt_string(&resource.meta.mime_type);
    let group = group_expr(resource, mcp);
    let requirements = super::requirements_expr(def, resource, mcp);
    let completions = completions_expr(def, resource, mcp);
    let meta = meta_expr(resource);

    quote! {
        {
            let __w = __wrapper.clone();
            #outer
            __routes.resources.push(#mcp::__macro_support::ResourceRoute {
                uri: #uri.into(),
                name: #name_str.into(),
                title: #title,
                description: #description,
                mime_type: #mime_type,
                requirements: #requirements,
                group: #group,
                completions: #completions,
                meta: #meta,
                invoke: ::std::sync::Arc::new(
                    move |__call: #mcp::__macro_support::ResourceCall|
                        -> #mcp::__macro_support::ResourceFuture {
                        let __w = __w.clone();
                        #inner
                        ::std::boxed::Box::pin(async move { #call })
                    },
                ),
            });
        }
    }
}

/// One `__routes.prompts.push(PromptRoute { ... })` statement for a prompt.
/// The advertised arguments are derived from the `Params<T>` input schema
/// (names, requiredness, per-property title/description); a prompt without
/// `Params<T>` advertises none.
fn generate_prompt_route(
    def: &McpRoutesImplDef,
    prompt: &McpTool,
    mcp: &TokenStream,
) -> TokenStream {
    let InvokeCall { outer, inner, call } = invoke_call(prompt);
    let name_str = prompt.tool_name();
    let title = opt_string(&prompt.meta.title);
    let description = opt_string(&tool_description(prompt));
    let group = group_expr(prompt, mcp);
    let requirements = super::requirements_expr(def, prompt, mcp);
    let completions = completions_expr(def, prompt, mcp);
    let meta = meta_expr(prompt);

    let arguments = match prompt.params_type() {
        Some(params_ty) => quote_spanned! {params_ty.span()=>
            #mcp::__macro_support::prompt_arguments_from_schema(
                &<#params_ty as #mcp::__macro_support::ToolParams>::input_schema(),
            )
        },
        None => quote! { ::std::vec::Vec::new() },
    };

    quote! {
        {
            let __w = __wrapper.clone();
            #outer
            __routes.prompts.push(#mcp::__macro_support::PromptRoute {
                name: #name_str.into(),
                title: #title,
                description: #description,
                arguments: #arguments,
                requirements: #requirements,
                group: #group,
                completions: #completions,
                meta: #meta,
                invoke: ::std::sync::Arc::new(
                    move |__call: #mcp::__macro_support::PromptCall|
                        -> #mcp::__macro_support::PromptFuture {
                        let __w = __w.clone();
                        #inner
                        ::std::boxed::Box::pin(async move { #call })
                    },
                ),
            });
        }
    }
}

/// The `completions: Vec<CompletionProvider>` field of a prompt/resource
/// route: one provider per `complete(arg = "method")` entry, each calling the
/// `#[completion]` method's invoke (parse() checked the method exists).
fn completions_expr(def: &McpRoutesImplDef, member: &McpTool, mcp: &TokenStream) -> TokenStream {
    let providers = member.meta.complete.iter().map(|(arg, provider)| {
        let arg = arg.to_string();
        let provider_name = provider.value();
        let provider_member = def
            .members
            .iter()
            .find(|m| m.kind == McpMemberKind::Completion && m.member.name == provider_name)
            .expect("parse() checked the completion provider exists");
        let InvokeCall { outer, inner, call } = invoke_call(provider_member);
        quote! {
            {
                let __w = __wrapper.clone();
                #outer
                #mcp::__macro_support::CompletionProvider {
                    argument: ::std::borrow::Cow::Borrowed(#arg),
                    invoke: ::std::sync::Arc::new(
                        move |__call: #mcp::__macro_support::Completion|
                            -> #mcp::__macro_support::CompletionFuture {
                            let __w = __w.clone();
                            #inner
                            ::std::boxed::Box::pin(async move { #call })
                        },
                    ),
                }
            }
        }
    });
    quote! { ::std::vec![#(#providers),*] }
}

/// The member description: explicit `description = "..."` override, or the
/// full doc comment (summary + body).
fn tool_description(tool: &McpTool) -> Option<String> {
    if tool.meta.description.is_some() {
        return tool.meta.description.clone();
    }
    tool.member.doc.clone()
}

fn opt_string(value: &Option<String>) -> TokenStream {
    match value {
        Some(s) => quote! { ::core::option::Option::Some(::std::string::String::from(#s)) },
        None => quote! { ::core::option::Option::None },
    }
}

/// A `Option<Cow<'static, str>>` literal — borrowed, so cloning the wire
/// metadata it ends up in never re-allocates the string.
fn opt_cow_str(value: &Option<String>) -> TokenStream {
    match value {
        Some(s) => quote! {
            ::core::option::Option::Some(::std::borrow::Cow::Borrowed(#s))
        },
        None => quote! { ::core::option::Option::None },
    }
}

fn opt_bool(value: Option<bool>) -> TokenStream {
    match value {
        Some(b) => quote! { ::core::option::Option::Some(#b) },
        None => quote! { ::core::option::Option::None },
    }
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

/// The `group: Option<McpGroup>` field of a member route.
fn group_expr(member: &McpTool, mcp: &TokenStream) -> TokenStream {
    match &member.group {
        Some(group) => {
            let name = &group.name;
            let ctor = if group.opt_in {
                quote! { opt_in }
            } else {
                quote! { new }
            };
            quote! {
                ::core::option::Option::Some(#mcp::__macro_support::McpGroup::#ctor(#name))
            }
        }
        None => quote! { ::core::option::Option::None },
    }
}
