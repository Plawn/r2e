//! Shared "transverse" (off-request) codegen: `#[scheduled]`, `#[consumer]`,
//! `#[intercept]`, and `#[post_construct]` wiring.
//!
//! These emitters were extracted verbatim from the `#[bean]` path
//! (`bean_attr.rs`) so `#[routes]` can delegate its duplicated transverse
//! codegen to the same machinery ("controller core IS a bean"). Everything
//! here is parameterized over the two axes that differ between a bean and a
//! controller core:
//!
//! - **impl target type** — a bean implements the traits for `Name`; a
//!   controller core implements them for `::std::sync::Arc<Name>` (cores live
//!   behind one `Arc` and may not be `Clone`, so the task/consumer closures
//!   clone the `Arc`). All emitters take the target as a [`TokenStream`].
//! - **slot access** — a bean reaches its decorator slot through
//!   `<Self as HasDecoSlot>::__r2e_deco_slot(self)` (a `SharedDecoSlot`); a
//!   controller core reads its `self.__r2e_decos` field (a `DecoSlot`, a
//!   distinct clones-empty type). Both the fill impl and the dispatch wrapper
//!   take the access expression as a parameter.
//!
//! The low-level parsers/emitters (`extract/scheduled.rs`,
//! `extract/consumer.rs`, `codegen/scheduled.rs`, `codegen/decorators.rs`)
//! stay the shared foundation; this module composes them.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, ImplItem, ItemImpl, ReturnType};

use crate::codegen::decorators::{intercept_field_idents, wrap_with_interceptor_refs};
use crate::codegen::scheduled::{overlap_policy_expr, schedule_config_expr, task_name};
use crate::crate_path::{r2e_core_path, r2e_events_path, r2e_executor_path, r2e_scheduler_path};
use crate::extract::consumer::strip_consumer_attrs;
use crate::type_utils::is_result_like;
use crate::types::{ConsumerKind, ScheduledConfig};

/// Strip the transverse wiring attributes (`#[consumer]`, `#[post_construct]`,
/// `#[scheduled]`, `#[intercept]`) from a method's attribute list, preserving
/// order. Used when re-emitting a source method's inner fn / dispatch wrapper.
fn strip_transverse_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    strip_consumer_attrs(attrs)
        .into_iter()
        .filter(|a| {
            !a.path().is_ident("post_construct")
                && !a.path().is_ident("scheduled")
                && !a.path().is_ident("intercept")
        })
        .collect()
}

// ── ScheduledSource ──────────────────────────────────────────────────────

/// One `#[scheduled]` method's data for [`scheduled_source_impl`].
pub(crate) struct ScheduledSourceMethod {
    pub fn_name: syn::Ident,
    pub config: ScheduledConfig,
    /// Whether the emitted call must be awaited: the source is `async` OR the
    /// method is intercepted (its dispatch wrapper is promoted to `async fn`).
    pub emitted_async: bool,
}

/// The per-method type-erased `ScheduledTaskDef` blocks (one per scheduled
/// method), parameterized by the instance expression whose `.clone()` each task
/// closure captures.
///
/// `instance` is a bean value (`self`, cloned by value) or a controller core's
/// `Arc` (cloned cheaply) — both `.clone()` correctly. `owner_name` seeds the
/// default task name (`<owner>_<method>`). Beans wrap these in a
/// [`ScheduledSource`] impl ([`scheduled_source_impl`]); controllers embed them
/// in the generated `Controller::scheduled_tasks_boxed` override (an
/// `Arc<Core>` is not a legal `ScheduledSource` impl target under the orphan
/// rule).
pub(crate) fn scheduled_task_defs(
    instance: &TokenStream,
    owner_name: &str,
    methods: &[ScheduledSourceMethod],
) -> Vec<TokenStream> {
    let sched_krate = r2e_scheduler_path();

    methods
        .iter()
        .map(|sm| {
            let fn_name = &sm.fn_name;
            let task_name = task_name(&sm.config, owner_name, &fn_name.to_string());
            let schedule_expr = schedule_config_expr(&sm.config, &sched_krate);
            let overlap_expr = overlap_policy_expr(sm.config.overlap, &sched_krate);

            let result_expr = if sm.emitted_async {
                quote! { __bean.#fn_name().await }
            } else {
                quote! { __bean.#fn_name() }
            };

            quote! {
                {
                    let __task_bean = #instance.clone();
                    let __task_def = #sched_krate::ScheduledTaskDef {
                        name: #task_name.to_string(),
                        schedule: #schedule_expr,
                        overlap: #overlap_expr,
                        state: (),
                        task: Box::new(move |(): ()| {
                            let __bean = __task_bean.clone();
                            Box::pin(async move {
                                #sched_krate::ScheduledResult::log_if_err(
                                    #result_expr,
                                    #task_name,
                                );
                            })
                        }),
                    };
                    let __boxed_task: Box<dyn #sched_krate::ScheduledTask> = Box::new(__task_def);
                    Box::new(__boxed_task) as Box<dyn std::any::Any + Send>
                }
            }
        })
        .collect()
}

/// Emit `impl ScheduledSource for <target>` from a list of scheduled methods.
///
/// `target` is the impl's Self type token (a `Clone` bean type). Returns empty
/// when `methods` is empty. The task closures clone `self`.
pub(crate) fn scheduled_source_impl(
    target: &TokenStream,
    owner_name: &str,
    methods: &[ScheduledSourceMethod],
) -> TokenStream {
    if methods.is_empty() {
        return quote! {};
    }

    let krate = r2e_core_path();
    let task_defs = scheduled_task_defs(&quote! { self }, owner_name, methods);

    quote! {
        impl #krate::ScheduledSource for #target {
            fn scheduled_tasks_boxed(
                &self,
                _ctx: &#krate::beans::BeanContext,
            ) -> Vec<Box<dyn std::any::Any + Send>> {
                vec![#(#task_defs),*]
            }
        }
    }
}

// ── EventSubscriber ──────────────────────────────────────────────────────

/// One `#[consumer]` method's data for [`event_subscriber_impl`].
pub(crate) struct ConsumerMethodDef {
    /// The event-bus field the consumer subscribes on.
    pub bus_field: syn::Ident,
    pub event_type: syn::Type,
    pub fn_name: syn::Ident,
    pub kind: ConsumerKind,
    pub topic: Option<String>,
    pub deserializer: Option<String>,
    pub filter: Option<String>,
    pub retry: Option<u32>,
    pub dlq: Option<String>,
}

/// The per-method subscribe/respond blocks (one per consumer method),
/// parameterized by the instance expression the closures clone.
///
/// `instance` is a bean value (`self`) or a controller core's `Arc` (`__core`)
/// — both `.clone()` correctly and deref to the concrete type for the method
/// call. `assoc_owner` is the path used to reference a custom `deserializer`
/// associated fn, which lives on the concrete type, not an `Arc` wrapper (bean:
/// `Self`; controller: `Name`). Beans wrap these in an [`EventSubscriber`] impl
/// ([`event_subscriber_impl`]); controllers embed them in the generated
/// `Controller::register_consumers` override.
pub(crate) fn event_subscribe_blocks(
    instance: &TokenStream,
    assoc_owner: &TokenStream,
    consumers: &[ConsumerMethodDef],
) -> Vec<TokenStream> {
    let events_krate = r2e_events_path();

    consumers
        .iter()
        .map(|cm| {
            let bus_field = &cm.bus_field;
            let event_type = &cm.event_type;
            let fn_name = &cm.fn_name;

            // Responder (non-`()` return): register via `EventBus::respond`.
            if let ConsumerKind::Responder { resp_type, fallible } = &cm.kind {
                let reply_map = if *fallible {
                    quote! { __reply }
                } else {
                    quote! { ::core::result::Result::<#resp_type, ::std::string::String>::Ok(__reply) }
                };
                return quote! {
                    {
                        let __bus = #instance.#bus_field.clone();
                        let __responder = #instance.clone();
                        let __handle = #events_krate::EventBus::respond::<#event_type, #resp_type, _, _, _>(
                            &__bus,
                            move |__envelope: #events_krate::EventEnvelope<#event_type>| {
                                let __this = __responder.clone();
                                async move {
                                    let __reply = __this.#fn_name(__envelope.event).await;
                                    #reply_map
                                }
                            },
                        ).await;
                        if let Err(__e) = __handle {
                            eprintln!("[r2e] Failed to register responder: {__e}");
                        }
                    }
                };
            }

            let register_topic = cm.topic.as_ref().map(|topic_str| {
                quote! {
                    #events_krate::EventBus::register_topic::<#event_type>(&__bus, #topic_str).await;
                }
            });

            let subscribe_call = if let Some(ref deser_fn) = cm.deserializer {
                let deser_ident = syn::Ident::new(deser_fn, proc_macro2::Span::call_site());
                quote! {
                    let __deser: #events_krate::backend::DeserializerFn = std::sync::Arc::new(#assoc_owner::#deser_ident);
                    #events_krate::EventBus::subscribe_with_deserializer::<#event_type, _, _>(&__bus, __deser, move |__envelope: #events_krate::EventEnvelope<#event_type>| {
                        let __this = __this.clone();
                        async move {
                            let __result = __this.#fn_name(__envelope.event).await;
                            ::core::convert::Into::<#events_krate::HandlerResult>::into(__result)
                        }
                    }).await
                }
            } else {
                quote! {
                    #events_krate::EventBus::subscribe(&__bus, move |__envelope: #events_krate::EventEnvelope<#event_type>| {
                        let __this = __this.clone();
                        async move {
                            let __result = __this.#fn_name(__envelope.event).await;
                            ::core::convert::Into::<#events_krate::HandlerResult>::into(__result)
                        }
                    }).await
                }
            };

            let has_filter = cm.filter.is_some();
            let filter_expr = if let Some(ref filter_fn) = cm.filter {
                let filter_ident = syn::Ident::new(filter_fn, proc_macro2::Span::call_site());
                quote! {
                    Some(std::sync::Arc::new({
                        let __this_for_filter = __this_orig.clone();
                        move |__meta: &#events_krate::EventMetadata| -> bool {
                            __this_for_filter.#filter_ident(__meta)
                        }
                    }) as #events_krate::EventFilter)
                }
            } else {
                quote! { None }
            };

            let has_retry = cm.retry.is_some();
            let retry_expr = if let Some(max_retries) = cm.retry {
                if let Some(ref dlq_topic) = cm.dlq {
                    quote! { Some(#events_krate::RetryPolicy::new(#max_retries).with_dlq(#dlq_topic)) }
                } else {
                    quote! { Some(#events_krate::RetryPolicy::new(#max_retries)) }
                }
            } else {
                quote! { None }
            };

            let configure_handler = if has_filter || has_retry {
                quote! {
                    if let Ok(ref __h) = __handle {
                        #events_krate::EventBus::configure_handler::<#event_type>(
                            &__bus_ref,
                            __h.id(),
                            #filter_expr,
                            #retry_expr,
                        ).await;
                    }
                }
            } else {
                quote! {}
            };

            quote! {
                {
                    let __bus = #instance.#bus_field.clone();
                    let __bus_ref = __bus.clone();
                    let __this_orig = #instance.clone();
                    let __this = #instance.clone();
                    #register_topic
                    let __handle = #subscribe_call;
                    #configure_handler
                    if let Err(__e) = __handle {
                        eprintln!("[r2e] Failed to subscribe consumer: {__e}");
                    }
                }
            }
        })
        .collect()
}

/// Emit `impl EventSubscriber for <target>` from a list of consumer methods.
///
/// `target` is the impl's Self type token (a `Clone` bean type). Custom
/// `deserializer` assoc fns are reached through `Self`. Returns empty when
/// `consumers` is empty.
pub(crate) fn event_subscriber_impl(
    target: &TokenStream,
    consumers: &[ConsumerMethodDef],
) -> TokenStream {
    if consumers.is_empty() {
        return quote! {};
    }

    let krate = r2e_core_path();
    let subscribe_blocks = event_subscribe_blocks(&quote! { self }, &quote! { Self }, consumers);

    quote! {
        impl #krate::EventSubscriber for #target {
            fn subscribe(self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                Box::pin(async move {
                    #(#subscribe_blocks)*
                })
            }
        }
    }
}

// ── Decorator container + fill impl ──────────────────────────────────────

/// One intercepted method's decorator-container field.
pub(crate) struct DecoFieldDef {
    /// The container field ident holding the prebuilt set (`__deco_<fn>`).
    pub field: syn::Ident,
    /// The decorator set's struct type (`DecoSet::ty()`).
    pub set_ty: syn::Ident,
    /// The decorator set's build function (`DecoSet::ctor_ident`).
    pub ctor: syn::Ident,
}

/// The shared controller-level interceptor set embedded in a transverse
/// container: a single `Arc<CtrlSet>` field (`__ctrl`) built once at fill time,
/// so every intercepted `#[scheduled]`/`#[consumer]` method of the controller
/// shares one impl-level interceptor instance. `None` for beans (which chain
/// their impl-level interceptors into each per-method set, unchanged).
pub(crate) struct CtrlContainerField {
    /// The set's struct type (`CtrlDecoSet::struct_ident`).
    pub set_ty: syn::Ident,
    /// The set's build function (`CtrlDecoSet::ctor_ident`).
    pub ctor: syn::Ident,
}

/// The container field name holding the shared controller-level interceptor set.
pub(crate) fn ctrl_container_field() -> syn::Ident {
    syn::Ident::new("__ctrl", proc_macro2::Span::call_site())
}

/// Emit the per-type decorator container struct and its `BeanDecoFill` impl.
///
/// `container` names the hidden struct; `target` is the fill impl's Self type
/// (bean: `Name`; controller core: `Arc<Name>`); `slot_access` is the
/// expression the fill impl calls `.fill(..)` on (bean:
/// `<Self as HasDecoSlot>::__r2e_deco_slot(self)`; controller core:
/// `self.__r2e_decos`). One field per intercepted method, each built from the
/// bean context at fill time. `ctrl` optionally adds the shared controller-level
/// interceptor set (`__ctrl: Arc<CtrlSet>`), built once at fill.
pub(crate) fn deco_container_and_fill(
    container: &syn::Ident,
    target: &TokenStream,
    slot_access: &TokenStream,
    fields: &[DecoFieldDef],
    ctrl: Option<&CtrlContainerField>,
) -> TokenStream {
    let krate = r2e_core_path();

    let mut container_fields: Vec<TokenStream> = Vec::new();
    let mut field_inits: Vec<TokenStream> = Vec::new();

    if let Some(c) = ctrl {
        let field = ctrl_container_field();
        let ty = &c.set_ty;
        let ctor = &c.ctor;
        container_fields.push(quote! { #field: ::std::sync::Arc<#ty> });
        field_inits.push(quote! { #field: ::std::sync::Arc::new(#ctor(__ctx)) });
    }
    for f in fields {
        let field = &f.field;
        let ty = &f.set_ty;
        let ctor = &f.ctor;
        container_fields.push(quote! { #field: #ty });
        field_inits.push(quote! { #field: #ctor(__ctx) });
    }

    quote! {
        #[allow(non_camel_case_types)]
        #[doc(hidden)]
        struct #container {
            #(#container_fields,)*
        }

        impl #krate::BeanDecoFill for #target {
            fn __r2e_fill_decos(&self, __ctx: &#krate::beans::BeanContext) {
                #slot_access.fill(#container {
                    #(#field_inits,)*
                });
            }
        }
    }
}

// ── Intercepted-method dispatch wrapper ──────────────────────────────────

/// Parameters for [`intercepted_dispatch_wrapper`].
pub(crate) struct DispatchWrapperParams {
    /// The container type whose slot entry holds this method's decorator set.
    pub container: syn::Ident,
    /// The container field holding this method's set (`__deco_<fn>`).
    pub field: syn::Ident,
    /// The expression yielding the decorator slot to `.get::<Container>()` on
    /// (bean: `<Self as HasDecoSlot>::__r2e_deco_slot(self)`; controller core:
    /// `self.__r2e_decos`).
    pub slot_access: TokenStream,
    /// The hidden inner fn name (bean: `__r2e_bean_<fn>_inner`).
    pub inner_name: syn::Ident,
    /// The interceptor-context owner name (`InterceptorContext::controller_name`).
    pub owner_name_str: String,
    /// Whether the SOURCE method is `async` (consumers always; sync scheduled
    /// sources are promoted). Governs whether the inner call is awaited and
    /// whether the wrapper is promoted to `async fn`.
    pub source_async: bool,
    /// The event parameter to forward on the inner call (consumers) — `None`
    /// for scheduled methods.
    pub event_param: Option<syn::PatType>,
    /// The number of METHOD-level `#[intercept]` sites on the method (the
    /// per-method decorator set's fields). May be 0 when the method runs only
    /// controller-level interceptors (see `ctrl_field_count`).
    pub intercept_count: usize,
    /// The number of shared controller-level (impl-level) interceptor sites,
    /// referenced through the container's `__ctrl` field (`&__decos.__ctrl.__ci*`).
    /// 0 for beans (they chain impl-level interceptors into each per-method set).
    pub ctrl_field_count: usize,
    /// The macro that promoted a sync source, named in the rustdoc note
    /// (bean: `#[bean]`; controller: `#[routes]`).
    pub origin_macro: &'static str,
}

/// Emit an intercepted `#[scheduled]`/`#[consumer]` method as a hidden renamed
/// inner fn + a dispatch wrapper that reads the prebuilt set from the
/// decorator slot and runs the interceptor chain, falling back to a bare inner
/// call when the slot is empty (unregistered instance). A sync scheduled
/// source is promoted to `async fn` (the chain must be awaited).
///
/// `transform_inner` runs on the cloned inner fn body — the bean path uses it
/// to rewrite struct literals that initialize the hidden slot field; other
/// callers pass a no-op.
pub(crate) fn intercepted_dispatch_wrapper(
    method: &syn::ImplItemFn,
    p: &DispatchWrapperParams,
    mut transform_inner: impl FnMut(&mut syn::Block),
) -> TokenStream {
    let krate = r2e_core_path();
    let fn_name = &method.sig.ident;
    let fn_name_str = fn_name.to_string();
    let method_fields = intercept_field_idents(p.intercept_count);
    let inner_name = &p.inner_name;

    // Inner fn: source body verbatim (attrs stripped), renamed & private.
    let mut inner_fn = method.clone();
    inner_fn.sig.ident = inner_name.clone();
    inner_fn.attrs = strip_transverse_attrs(inner_fn.attrs);
    inner_fn.attrs.push(syn::parse_quote!(#[doc(hidden)]));
    inner_fn.vis = syn::Visibility::Inherited;
    transform_inner(&mut inner_fn.block);

    // Wrapper signature: keep vis + params + output; promote sync sources.
    let vis = &method.vis;
    let mut sig = method.sig.clone();
    let promotion_doc = if p.source_async {
        quote! {}
    } else {
        sig.asyncness = Some(Default::default());
        let note = format!(
            "*R2E:* promoted to `async fn` by `{}` — this sync `#[scheduled]` \
             method has `#[intercept]` sites, and the chain (which must be awaited) \
             runs on direct calls too. Call with `.await`.",
            p.origin_macro
        );
        quote! {
            #[doc = ""]
            #[doc = #note]
        }
    };
    let wrapper_attrs = strip_transverse_attrs(method.attrs.clone());

    // Inner call: forward the event param for consumers.
    let arg_forward = p.event_param.as_ref().map(|pt| {
        let pat = &pt.pat;
        quote! { #pat }
    });
    let inner_call = if p.source_async {
        quote! { self.#inner_name(#arg_forward).await }
    } else {
        quote! { self.#inner_name(#arg_forward) }
    };

    // Combined interceptor refs: shared controller-level (impl-level) ones
    // outermost — read from the container's `__ctrl` field so every intercepted
    // transverse method shares a single instance — then the per-method ones.
    let ctrl_field = ctrl_container_field();
    // Controller-level set fields are `__ci0..` (see `ctrl_deco_set`).
    let mut interceptor_refs: Vec<TokenStream> = (0..p.ctrl_field_count)
        .map(|i| {
            let f = quote::format_ident!("__ci{}", i);
            quote! { &__decos.#ctrl_field.#f }
        })
        .collect();
    interceptor_refs.extend(method_fields.iter().map(|f| quote! { &__deco.#f }));

    let chain = wrap_with_interceptor_refs(
        inner_call.clone(),
        &fn_name_str,
        &p.owner_name_str,
        &interceptor_refs,
        &krate,
    );

    let container = &p.container;
    let field = &p.field;
    let slot_access = &p.slot_access;
    // Bind the per-method set only when there are method-level interceptors;
    // a controller-level-only method reads just `__decos.__ctrl`.
    let method_bind = (p.intercept_count > 0).then(|| quote! { let __deco = &__decos.#field; });

    quote! {
        #inner_fn

        #(#wrapper_attrs)*
        #promotion_doc
        #vis #sig {
            match #slot_access.get::<#container>() {
                Some(__decos) => {
                    #method_bind
                    #chain
                }
                None => #inner_call,
            }
        }
    }
}

// ── #[async_exec] pool-submission wrapper ────────────────────────────────

/// Emit an `#[async_exec]` method as a hidden renamed inner `async fn`
/// (`__r2e_async_<name>_inner`, body verbatim) plus a synchronous wrapper
/// that clones `self`, captures the named executor field, and submits the
/// inner body to the pool, returning
/// `Result<JobHandle<T>, RejectedError>` instead of `T`.
///
/// Shared by `#[routes]` (controller-core methods) and `#[bean]` (bean
/// methods) — both targets are `Clone` values whose executor field holds a
/// `PoolExecutor`-compatible bean. `method` must already have the
/// `#[async_exec]` attribute stripped; the remaining attributes are kept on
/// both the inner fn and the wrapper.
///
/// `transform_inner` runs on the cloned inner fn body — the bean path uses it
/// to rewrite struct literals that initialize the hidden slot field; other
/// callers pass a no-op.
pub(crate) fn async_exec_method(
    method: &syn::ImplItemFn,
    executor_field: &syn::Ident,
    mut transform_inner: impl FnMut(&mut syn::Block),
) -> TokenStream {
    let exec_krate = r2e_executor_path();
    let original_sig = &method.sig;
    let original_name = &original_sig.ident;
    let inner_name = quote::format_ident!("__r2e_async_{}_inner", original_name);

    let mut inner_fn = method.clone();
    inner_fn.sig.ident = inner_name.clone();
    transform_inner(&mut inner_fn.block);

    let return_ty: TokenStream = match &original_sig.output {
        ReturnType::Default => quote! { () },
        ReturnType::Type(_, ty) => quote! { #ty },
    };

    // Wrapper params are re-bound as plain `ident: Type` (a destructuring
    // pattern like `(a, b): (u32, u32)` stays on the inner fn, which receives
    // the whole value); the wrapper forwards each ident to the inner call.
    let typed_inputs: Vec<&syn::PatType> = original_sig
        .inputs
        .iter()
        .filter_map(|a| {
            if let FnArg::Typed(pt) = a {
                Some(pt)
            } else {
                None
            }
        })
        .collect();
    let arg_idents: Vec<syn::Ident> = typed_inputs
        .iter()
        .enumerate()
        .map(|(i, pt)| match &*pt.pat {
            syn::Pat::Ident(pi) => pi.ident.clone(),
            _ => quote::format_ident!("__arg_{}", i),
        })
        .collect();
    let wrapper_params: Vec<TokenStream> = typed_inputs
        .iter()
        .zip(&arg_idents)
        .map(|(pt, ident)| {
            let ty = &pt.ty;
            quote! { #ident: #ty }
        })
        .collect();

    let attrs = &method.attrs;
    let vis = &method.vis;
    let generics = &original_sig.generics;
    let where_clause = &original_sig.generics.where_clause;

    quote! {
        #inner_fn

        #(#attrs)*
        #vis fn #original_name #generics (
            &self,
            #(#wrapper_params),*
        ) -> ::core::result::Result<#exec_krate::JobHandle<#return_ty>, #exec_krate::RejectedError> #where_clause {
            let __self = ::core::clone::Clone::clone(self);
            self.#executor_field.submit(async move {
                __self.#inner_name(#(#arg_idents),*).await
            })
        }
    }
}

// ── PostConstruct ────────────────────────────────────────────────────────

/// One `#[post_construct]` method's dispatch shape.
pub(crate) struct PostConstructMethod {
    pub fn_name: syn::Ident,
    pub is_async: bool,
    pub returns_result: bool,
}

/// Scan all `&self` methods in an impl block for `#[post_construct]`.
pub(crate) fn scan_post_construct_methods(
    item_impl: &ItemImpl,
) -> syn::Result<Vec<PostConstructMethod>> {
    let mut methods = Vec::new();

    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            let has_self = method
                .sig
                .inputs
                .iter()
                .any(|arg| matches!(arg, FnArg::Receiver(_)));
            if !has_self {
                continue;
            }

            let has_attr = method
                .attrs
                .iter()
                .any(|a| a.path().is_ident("post_construct"));
            if !has_attr {
                continue;
            }

            let param_count = method.sig.inputs.len();
            if param_count > 1 {
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    "#[post_construct] method must take only `&self` — no additional parameters",
                ));
            }

            let is_async = method.sig.asyncness.is_some();
            let returns_result = match &method.sig.output {
                ReturnType::Default => false,
                ReturnType::Type(_, ty) => is_result_like(ty),
            };

            methods.push(PostConstructMethod {
                fn_name: method.sig.ident.clone(),
                is_async,
                returns_result,
            });
        }
    }

    Ok(methods)
}

/// Emit `impl PostConstruct for <target>` from a list of post-construct
/// methods. `target` is the impl's Self type token. Returns empty when
/// `methods` is empty.
pub(crate) fn post_construct_impl(
    target: &TokenStream,
    methods: &[PostConstructMethod],
) -> TokenStream {
    if methods.is_empty() {
        return quote! {};
    }

    let krate = r2e_core_path();

    let calls: Vec<TokenStream> = methods
        .iter()
        .map(|m| {
            let fn_name = &m.fn_name;
            match (m.is_async, m.returns_result) {
                (true, true) => quote! { self.#fn_name().await?; },
                (true, false) => quote! { self.#fn_name().await; },
                (false, true) => quote! { self.#fn_name()?; },
                (false, false) => quote! { self.#fn_name(); },
            }
        })
        .collect();

    quote! {
        impl #krate::beans::PostConstruct for #target {
            fn post_construct(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + '_>> {
                Box::pin(async move {
                    #(#calls)*
                    Ok(())
                })
            }
        }
    }
}
