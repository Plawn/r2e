use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::visit_mut::VisitMut;
use syn::{parse_macro_input, FnArg, ImplItem, Item, ItemImpl, ItemStruct, ReturnType, Type};

use crate::codegen::decorators::{deps_fold_from_base, generate_named_deco_items, DecoSet};
use crate::codegen::transverse::{
    self, ConsumerMethodDef, DecoFieldDef, DispatchWrapperParams, ScheduledSourceMethod,
};
use crate::crate_path::r2e_core_path;
use crate::extract::async_exec::{
    extract_async_exec, strip_async_exec_attrs, validate_async_exec_method,
    AsyncExecHost, ASYNC_EXEC_INTERCEPT_MSG,
};
use crate::extract::consumer::{
    classify_consumer_return, extract_consumer, extract_event_type_from_arc, strip_consumer_attrs,
};
use crate::extract::route::extract_intercept_fns;
use crate::extract::scheduled::extract_scheduled;
use crate::hash_tokens::hash_token_stream;
use crate::type_list_gen::build_tcons_type;
use crate::types::ConsumerKind;
use crate::type_utils::{
    named_bean_newtype_ident, parse_config_field, parse_config_section_prefix, parse_inject_name,
};

/// Parsed `#[bean(...)]` arguments.
struct BeanArgs {
    /// When `true`, the bean is marked for lazy initialization.
    lazy: bool,
}

impl BeanArgs {
    fn parse(args: TokenStream) -> syn::Result<Self> {
        let mut lazy = false;
        if !args.is_empty() {
            let parser = syn::meta::parser(|meta| {
                if meta.path.is_ident("lazy") {
                    lazy = true;
                    Ok(())
                } else {
                    Err(meta.error("expected `lazy`"))
                }
            });
            syn::parse::Parser::parse(parser, args)?;
        }
        Ok(Self { lazy })
    }
}

/// Parsed consumer method data from a `#[bean]` impl block.
struct BeanConsumerMethod {
    config: crate::extract::consumer::ConsumerConfig,
    event_type: syn::Type,
    kind: ConsumerKind,
    fn_name: syn::Ident,
    /// Effective `#[intercept(...)]` sites (impl-level first, then method-level).
    intercept_fns: Vec<syn::Expr>,
}

/// Parsed `#[async_exec]` method data from a `#[bean]` impl block.
struct BeanAsyncExecMethod {
    fn_name: syn::Ident,
    /// Name of the bean field holding the `PoolExecutor`. Default: `executor`.
    executor_field: syn::Ident,
}

/// Parsed scheduled method data from a `#[bean]` impl block.
struct BeanScheduledMethod {
    config: crate::types::ScheduledConfig,
    fn_name: syn::Ident,
    is_async: bool,
    /// Effective `#[intercept(...)]` sites (impl-level first, then method-level).
    intercept_fns: Vec<syn::Expr>,
}

/// An intercepted `#[scheduled]`/`#[consumer]` method: its prebuilt decorator
/// set (a hidden struct + ctor) and the metadata the wrapper codegen needs.
struct BeanInterceptedMethod {
    fn_name: syn::Ident,
    /// Whether the *source* method is async. Sync scheduled sources are
    /// promoted to `async fn` (the chain must be awaited); consumers are
    /// always async.
    source_async: bool,
    /// The event parameter, present only for `#[consumer]` wrappers. The
    /// wrapper keeps the source signature (including the return type) verbatim
    /// via `method.sig.clone()`, so only the parameter needs forwarding.
    event_param: Option<syn::PatType>,
    intercept_fns: Vec<syn::Expr>,
    set: DecoSet,
}

pub fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as Item);
    match item {
        Item::Struct(item_struct) => {
            if !args.is_empty() {
                return syn::Error::new_spanned(
                    &item_struct.ident,
                    "#[bean] on a struct takes no arguments (`lazy` applies to the impl block)",
                )
                .to_compile_error()
                .into();
            }
            match expand_struct(item_struct) {
                Ok(ts) => ts.into(),
                Err(err) => err.to_compile_error().into(),
            }
        }
        Item::Impl(item_impl) => {
            let bean_args = match BeanArgs::parse(args) {
                Ok(a) => a,
                Err(err) => return err.to_compile_error().into(),
            };
            expand_impl(item_impl, &bean_args).into()
        }
        other => syn::Error::new_spanned(
            other,
            "#[bean] is only valid on a `struct` (to inject the decorator slot) \
             or an `impl` block (to declare the constructor / scheduled / consumer methods)",
        )
        .to_compile_error()
        .into(),
    }
}

/// `#[bean]` on a struct: inject the hidden decorator slot field and implement
/// [`HasDecoSlot`]. The slot is what lets a `#[bean]` impl's intercepted
/// `#[scheduled]`/`#[consumer]` methods self-intercept on direct calls.
fn expand_struct(mut item_struct: ItemStruct) -> syn::Result<TokenStream2> {
    let krate = r2e_core_path();
    let name = &item_struct.ident;
    let (impl_generics, ty_generics, where_clause) = item_struct.generics.split_for_impl();

    match &mut item_struct.fields {
        syn::Fields::Named(named) => {
            named.named.push(syn::parse_quote! {
                #[doc(hidden)]
                pub __r2e_decos: #krate::SharedDecoSlot
            });
        }
        _ => {
            return Err(syn::Error::new_spanned(
                &item_struct.ident,
                "#[bean] on a struct requires named fields to hold the hidden decorator slot — \
                 give the struct braces (`struct Foo { .. }`), even if empty (`struct Foo {}`)",
            ));
        }
    }

    let has_deco_slot = quote! {
        impl #impl_generics #krate::HasDecoSlot for #name #ty_generics #where_clause {
            fn __r2e_deco_slot(&self) -> &#krate::SharedDecoSlot {
                &self.__r2e_decos
            }
        }
    };

    Ok(quote! {
        #item_struct
        #has_deco_slot
    })
}

fn expand_impl(item_impl: ItemImpl, bean_args: &BeanArgs) -> TokenStream2 {
    match generate(&item_impl, bean_args) {
        Ok(generated) => {
            let cleaned_impl = emit_cleaned_impl(&item_impl, &generated);
            let bean_impl = &generated.bean_impl;
            quote! {
                #cleaned_impl
                #bean_impl
            }
        }
        Err(err) => err.to_compile_error(),
    }
}

/// Everything `generate` hands back to the cleaned-impl emission.
struct GeneratedBean {
    /// The hidden trait impls (Bean/Registrable/subscriber/scheduled/...).
    bean_impl: TokenStream2,
    /// Intercepted `#[scheduled]`/`#[consumer]` methods (split into inner +
    /// dispatch wrapper by the cleaned-impl emission).
    intercepted: Vec<BeanInterceptedMethod>,
    /// `#[async_exec]` methods (split into inner + pool-submission wrapper).
    async_exec: Vec<BeanAsyncExecMethod>,
    /// Whether the impl block carries an impl-level `#[intercept]`.
    impl_intercepts_present: bool,
}

fn generate(item_impl: &ItemImpl, bean_args: &BeanArgs) -> syn::Result<GeneratedBean> {
    // Extract the Self type from the impl block.
    let self_ty = &item_impl.self_ty;
    let type_ident = type_ident(self_ty);

    // Find the constructor: a method that returns Self and has no self receiver.
    let (constructor, is_async) = find_constructor(item_impl)?;

    // Extract parameter types and generate dependency list + build args.
    let mut dep_type_ids = Vec::new();
    let mut dep_types: Vec<TokenStream2> = Vec::new();
    let mut build_args = Vec::new();
    let mut config_key_entries = Vec::new();
    let mut has_config = false;

    let fn_name = &constructor.sig.ident;
    let type_name_str = quote!(#self_ty).to_string();

    for (i, arg) in constructor.sig.inputs.iter().enumerate() {
        match arg {
            FnArg::Receiver(_) => {
                return Err(syn::Error::new_spanned(
                    arg,
                    "#[bean] constructor must be a static associated function (no `self` parameter):\n\
                     \n  fn new(dep: MyDependency) -> Self {\n      Self { dep }\n  }",
                ));
            }
            FnArg::Typed(pat_type) => {
                let ty = &*pat_type.ty;
                let arg_name = syn::Ident::new(&format!("__arg_{}", i), proc_macro2::Span::call_site());

                let inject_name = parse_inject_name(&pat_type.attrs)?;

                let config_attr = pat_type.attrs.iter().find(|a| a.path().is_ident("config"));
                let config_section_attr = pat_type.attrs.iter().find(|a| a.path().is_ident("config_section"));

                if let Some(name) = inject_name {
                    let newtype_ident = named_bean_newtype_ident(&name, ty);
                    dep_type_ids.push(quote! { (std::any::TypeId::of::<#newtype_ident>(), std::any::type_name::<#newtype_ident>()) });
                    dep_types.push(quote! { #newtype_ident });
                    build_args.push(quote! { let #arg_name: #ty = ctx.get::<#newtype_ident>().0; });
                } else if let Some(attr) = config_section_attr {
                    let prefix_str = parse_config_section_prefix(attr)?;
                    let krate = r2e_core_path();
                    build_args.push(quote! {
                        let #arg_name: #ty = #krate::config::ConfigProperties::from_config(&__r2e_config, Some(#prefix_str)).unwrap_or_else(|e| {
                            panic!(
                                "Configuration error in bean `{}`: config section '{}' — {}",
                                #type_name_str, #prefix_str, e
                            )
                        });
                    });
                    has_config = true;
                } else if let Some(attr) = config_attr {
                    let (key_str, ty_name_str) = parse_config_field(attr, ty)?;
                    let krate = r2e_core_path();
                    let is_option = crate::type_utils::is_option_type(ty);
                    // Emit a `config_keys()` entry for EVERY key (required and
                    // optional) so dev-reload fingerprints the value; `required`
                    // gates presence validation.
                    let required = !is_option;
                    config_key_entries.push(quote! { (#key_str, #ty_name_str, #required) });
                    let owner = format!("bean `{type_name_str}`");
                    let expr = crate::field_resolver::config_resolve_expr(
                        &quote! { __r2e_config }, &key_str, Some(ty), &owner, is_option, &krate,
                    );
                    build_args.push(quote! { let #arg_name: #ty = #expr; });
                    has_config = true;
                } else {
                    dep_type_ids.push(quote! { (std::any::TypeId::of::<#ty>(), std::any::type_name::<#ty>()) });
                    dep_types.push(quote! { #ty });
                    build_args.push(quote! { let #arg_name: #ty = ctx.get::<#ty>(); });
                }
            }
        }
    }

    if has_config {
        let krate = r2e_core_path();
        dep_type_ids.push(
            quote! { (std::any::TypeId::of::<#krate::config::R2eConfig>(), std::any::type_name::<#krate::config::R2eConfig>()) },
        );
        dep_types.push(quote! { #krate::config::R2eConfig });
    }

    let arg_forwards: Vec<_> = (0..build_args.len())
        .map(|i| {
            let arg_name = syn::Ident::new(&format!("__arg_{}", i), proc_macro2::Span::call_site());
            quote! { #arg_name }
        })
        .collect();

    let krate = r2e_core_path();
    let base_deps_type = build_tcons_type(&dep_types, &krate);

    let build_version = hash_token_stream(&quote! { #constructor });

    let config_prelude = if has_config {
        quote! { let __r2e_config: #krate::config::R2eConfig = ctx.get::<#krate::config::R2eConfig>(); }
    } else {
        quote! {}
    };

    let config_keys_fn = if config_key_entries.is_empty() {
        quote! {}
    } else {
        quote! {
            fn config_keys() -> Vec<(&'static str, &'static str, bool)> {
                vec![#(#config_key_entries),*]
            }
        }
    };

    // Impl-level `#[intercept(...)]` — applies to every scheduled/consumer
    // method, running BEFORE method-level interceptors (same order as
    // controller-level interceptors).
    let impl_intercepts = extract_intercept_fns(&item_impl.attrs)?;

    // Reject `#[intercept]` on plain bean methods (neither scheduled nor
    // consumer): interceptors only make sense on the two off-request wiring
    // kinds, whose dispatch wrappers can run the chain.
    reject_stray_intercepts(item_impl)?;

    // Scan for #[scheduled] methods FIRST (its scheduled+consumer conflict
    // check must fire before the consumer scan's signature validation).
    let scheduled_methods = scan_scheduled_methods(item_impl, &impl_intercepts)?;
    if bean_args.lazy && !scheduled_methods.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_impl.self_ty,
            "#[bean(lazy)] does not yet support #[scheduled] methods — remove one or the other",
        ));
    }

    let consumer_methods = scan_consumer_methods(item_impl, &impl_intercepts)?;
    if bean_args.lazy && !consumer_methods.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_impl.self_ty,
            "#[bean(lazy)] does not yet support #[consumer] methods — remove one or the other",
        ));
    }

    // `#[async_exec]` is pure per-method codegen (no registration hook), so it
    // composes with `lazy` and needs nothing at `build_state()`.
    let async_exec_methods = scan_async_exec_methods(item_impl)?;

    let pc_methods = transverse::scan_post_construct_methods(item_impl)?;
    if bean_args.lazy && !pc_methods.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_impl.self_ty,
            "#[bean(lazy)] does not yet support #[post_construct] — remove one or the other",
        ));
    }

    // An impl-level `#[intercept]` applies only to scheduled/consumer methods;
    // with none present it is a silent no-op (and would force the constructor
    // literal rewrite without a matching wrapper). Fail loud on the attribute.
    if !impl_intercepts.is_empty() && scheduled_methods.is_empty() && consumer_methods.is_empty() {
        let attr = item_impl
            .attrs
            .iter()
            .find(|a| a.path().is_ident("intercept"))
            .expect("impl_intercepts non-empty implies an #[intercept] attr");
        return Err(syn::Error::new_spanned(
            attr,
            "impl-level #[intercept] on a #[bean] impl requires at least one #[scheduled] \
             or #[consumer] method — it applies to those methods, and there are none here",
        ));
    }

    // ── Decorator sets: one hidden struct + ctor per intercepted method ──
    let mut deco_module_items: Vec<TokenStream2> = Vec::new();
    let mut intercepted: Vec<BeanInterceptedMethod> = Vec::new();
    let mut all_intercept_exprs: Vec<syn::Expr> = Vec::new();

    for sm in &scheduled_methods {
        if sm.intercept_fns.is_empty() {
            continue;
        }
        all_intercept_exprs.extend(sm.intercept_fns.iter().cloned());
        let (items, set) = generate_named_deco_items(
            &type_ident,
            "BeanSched",
            &sm.fn_name,
            &[],
            &sm.intercept_fns.iter().collect::<Vec<_>>(),
            quote! {},
        );
        deco_module_items.push(items);
        if let Some(set) = set {
            intercepted.push(BeanInterceptedMethod {
                fn_name: sm.fn_name.clone(),
                source_async: sm.is_async,
                event_param: None,
                intercept_fns: sm.intercept_fns.clone(),
                set,
            });
        }
    }

    for cm in &consumer_methods {
        if cm.intercept_fns.is_empty() {
            continue;
        }
        all_intercept_exprs.extend(cm.intercept_fns.iter().cloned());
        let (items, set) = generate_named_deco_items(
            &type_ident,
            "BeanCons",
            &cm.fn_name,
            &[],
            &cm.intercept_fns.iter().collect::<Vec<_>>(),
            quote! {},
        );
        deco_module_items.push(items);
        if let Some(set) = set {
            // Recover the event param from the impl method (the wrapper keeps
            // the full signature verbatim, so only the param needs forwarding).
            let event_param = consumer_event_param(item_impl, &cm.fn_name);
            intercepted.push(BeanInterceptedMethod {
                fn_name: cm.fn_name.clone(),
                source_async: true,
                event_param,
                intercept_fns: cm.intercept_fns.clone(),
                set,
            });
        }
    }

    let has_decos = !intercepted.is_empty();

    // Per-bean decorator container + BeanDecoFill impl (only when at least one
    // method actually has an inferable interceptor set). The bean reaches its
    // slot through `HasDecoSlot` (a `SharedDecoSlot`).
    let deco_fill_impl = if has_decos {
        let container = bean_deco_container_ident(&type_ident);
        let fields: Vec<DecoFieldDef> = intercepted
            .iter()
            .map(|im| DecoFieldDef {
                field: bean_deco_field_ident(&im.fn_name),
                set_ty: im.set.ty().clone(),
                ctor: im.set.ctor_ident.clone(),
            })
            .collect();
        let slot_access = quote! { <Self as #krate::HasDecoSlot>::__r2e_deco_slot(self) };
        transverse::deco_container_and_fill(&container, &quote! { #self_ty }, &slot_access, &fields)
    } else {
        quote! {}
    };

    // Registrable/Bean deps: constructor deps ++ every distinct decorator
    // spec's `Deps`. The runtime `dependencies()` vec stays constructor-only,
    // so decorator deps are compile-checked (at `.register::<T>()`) without
    // affecting the topological sort.
    let deps_type = if all_intercept_exprs.is_empty() {
        base_deps_type.clone()
    } else {
        deps_fold_from_base(base_deps_type.clone(), all_intercept_exprs.iter())
    };

    let sched_defs: Vec<ScheduledSourceMethod> = scheduled_methods
        .iter()
        .map(|sm| ScheduledSourceMethod {
            fn_name: sm.fn_name.clone(),
            config: sm.config.clone(),
            // Intercepted methods self-intercept in their dispatch wrapper
            // (sync sources are promoted to `async fn`), so the emitted call is
            // awaited whenever the source is async OR it has an inferable
            // interceptor set (a non-inferable set already emits a
            // compile_error and skips the wrapper — don't stack an
            // await-on-sync error on top).
            emitted_async: sm.is_async
                || intercepted.iter().any(|im| im.fn_name == sm.fn_name),
        })
        .collect();
    let scheduled_source_impl =
        transverse::scheduled_source_impl(&quote! { #self_ty }, &type_ident.to_string(), &sched_defs);

    let consumer_defs: Vec<ConsumerMethodDef> = consumer_methods
        .iter()
        .map(|cm| ConsumerMethodDef {
            bus_field: syn::Ident::new(&cm.config.bus_field, proc_macro2::Span::call_site()),
            event_type: cm.event_type.clone(),
            fn_name: cm.fn_name.clone(),
            kind: cm.kind.clone(),
            topic: cm.config.topic.clone(),
            deserializer: cm.config.deserializer.clone(),
            filter: cm.config.filter.clone(),
            retry: cm.config.retry,
            dlq: cm.config.dlq.clone(),
        })
        .collect();
    // A bean implements `EventSubscriber` for its own type, so custom
    // `deserializer` associated fns are reached through `Self`.
    let subscriber_impl =
        transverse::event_subscriber_impl(&quote! { #self_ty }, &consumer_defs);

    let post_construct_impl = transverse::post_construct_impl(&quote! { #self_ty }, &pc_methods);

    let after_register_fn = if !pc_methods.is_empty()
        || !scheduled_methods.is_empty()
        || !consumer_methods.is_empty()
        || has_decos
    {
        let pc_hook = (!pc_methods.is_empty())
            .then(|| quote! { registry.register_post_construct::<Self>(); });
        let sched_hook = (!scheduled_methods.is_empty())
            .then(|| quote! { registry.register_scheduled_source::<Self>(); });
        let sub_hook = (!consumer_methods.is_empty())
            .then(|| quote! { registry.register_event_subscriber::<Self>(); });
        let deco_hook =
            has_decos.then(|| quote! { registry.register_deco_fill::<Self>(); });
        quote! {
            fn after_register(registry: &mut #krate::beans::BeanRegistry) {
                #pc_hook
                #sched_hook
                #sub_hook
                #deco_hook
            }
        }
    } else {
        quote! {}
    };

    let lazy_const = bean_args.lazy;

    let bean_impl = if is_async {
        quote! {
            impl #krate::beans::AsyncBean for #self_ty {
                type Deps = #deps_type;
                const LAZY: bool = #lazy_const;
                fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                    vec![#(#dep_type_ids),*]
                }
                #config_keys_fn
                const BUILD_VERSION: u64 = #build_version;
                async fn build(ctx: &#krate::beans::BeanContext) -> Self {
                    #config_prelude
                    #(#build_args)*
                    Self::#fn_name(#(#arg_forwards),*).await
                }
                #after_register_fn
            }
        }
    } else {
        quote! {
            impl #krate::beans::Bean for #self_ty {
                type Deps = #deps_type;
                const LAZY: bool = #lazy_const;
                fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                    vec![#(#dep_type_ids),*]
                }
                #config_keys_fn
                const BUILD_VERSION: u64 = #build_version;
                fn build(ctx: &#krate::beans::BeanContext) -> Self {
                    #config_prelude
                    #(#build_args)*
                    Self::#fn_name(#(#arg_forwards),*)
                }
                #after_register_fn
            }
        }
    };

    let register_call = if is_async {
        quote! { registry.register_async::<Self>(); }
    } else {
        quote! { registry.register::<Self>(); }
    };
    let registrable_impl = quote! {
        impl #krate::beans::Registrable for #self_ty {
            type Provided = Self;
            type Deps = #deps_type;

            fn register_into(registry: &mut #krate::beans::BeanRegistry) {
                #register_call
            }
        }
    };

    let deco_module_items = quote! { #(#deco_module_items)* };

    let out = quote! {
        #deco_module_items
        #deco_fill_impl
        #bean_impl
        #registrable_impl
        #post_construct_impl
        #subscriber_impl
        #scheduled_source_impl
    };

    Ok(GeneratedBean {
        bean_impl: out,
        intercepted,
        async_exec: async_exec_methods,
        impl_intercepts_present: !impl_intercepts.is_empty(),
    })
}

/// The last path-segment identifier of the impl type, used to name hidden
/// per-bean items (decorator container, decorator sets).
fn type_ident(self_ty: &syn::Type) -> syn::Ident {
    if let Type::Path(tp) = self_ty {
        if let Some(seg) = tp.path.segments.last() {
            return seg.ident.clone();
        }
    }
    format_ident!("Bean")
}

fn bean_deco_container_ident(name: &syn::Ident) -> syn::Ident {
    format_ident!("__R2eBeanDecos_{}", name)
}

fn bean_deco_field_ident(fn_name: &syn::Ident) -> syn::Ident {
    format_ident!("__deco_{}", fn_name)
}

/// Recover a consumer method's event parameter from the impl block (the scan
/// structs don't keep the whole signature; the wrapper forwards it to the
/// inner call).
fn consumer_event_param(item_impl: &ItemImpl, fn_name: &syn::Ident) -> Option<syn::PatType> {
    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            if &method.sig.ident == fn_name {
                return method.sig.inputs.iter().find_map(|arg| match arg {
                    FnArg::Typed(pt) => Some(pt.clone()),
                    _ => None,
                });
            }
        }
    }
    None
}

/// Reject `#[intercept]` on a `&self` method that is neither `#[scheduled]`
/// nor `#[consumer]` — there is no dispatch wrapper to run the chain there.
fn reject_stray_intercepts(item_impl: &ItemImpl) -> syn::Result<()> {
    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            let has_intercept = method.attrs.iter().any(|a| a.path().is_ident("intercept"));
            if !has_intercept {
                continue;
            }
            let is_scheduled = method.attrs.iter().any(|a| a.path().is_ident("scheduled"));
            let is_consumer = method.attrs.iter().any(|a| a.path().is_ident("consumer"));
            if !is_scheduled && !is_consumer {
                let is_async_exec =
                    method.attrs.iter().any(|a| a.path().is_ident("async_exec"));
                if is_async_exec {
                    // Shared message with the controller path; spanned on the
                    // whole signature here (this fires before the async_exec
                    // scan).
                    return Err(syn::Error::new_spanned(
                        &method.sig,
                        ASYNC_EXEC_INTERCEPT_MSG,
                    ));
                }
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    "#[intercept] on a bean method is only supported on #[scheduled]/#[consumer] \
                     methods — a plain method has no dispatch wrapper to run the interceptor chain",
                ));
            }
        }
    }
    Ok(())
}

/// Scan the impl block for `#[async_exec]` methods.
fn scan_async_exec_methods(item_impl: &ItemImpl) -> syn::Result<Vec<BeanAsyncExecMethod>> {
    let mut methods = Vec::new();

    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            let Some(cfg) = extract_async_exec(&method.attrs)? else {
                continue;
            };

            // All async_exec validation (conflict matrix, intercept, asyncness,
            // receiver) lives in the shared validator so the bean and
            // controller hosts stay in lockstep. The intercept case is already
            // caught earlier by `reject_stray_intercepts` (whole-signature
            // span); this re-check is a harmless backstop.
            validate_async_exec_method(&method.attrs, &method.sig, AsyncExecHost::Bean)?;

            methods.push(BeanAsyncExecMethod {
                fn_name: method.sig.ident.clone(),
                executor_field: cfg.executor_field,
            });
        }
    }

    Ok(methods)
}

/// Scan all `&self` methods in the impl block for `#[consumer(bus = "...")]`.
fn scan_consumer_methods(
    item_impl: &ItemImpl,
    impl_intercepts: &[syn::Expr],
) -> syn::Result<Vec<BeanConsumerMethod>> {
    let mut consumers = Vec::new();

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

            if let Some(config) = extract_consumer(&method.attrs)? {
                let event_param = method
                    .sig
                    .inputs
                    .iter()
                    .find_map(|arg| match arg {
                        FnArg::Typed(pt) => Some(pt),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        syn::Error::new(
                            method.sig.ident.span(),
                            "#[consumer] method must have an event parameter typed as Arc<EventType>:\n\
                             \n  #[consumer(bus = \"event_bus\")]\n\
                             \n  async fn on_event(&self, event: Arc<MyEvent>) { }",
                        )
                    })?;
                let event_type = extract_event_type_from_arc(&event_param.ty)?;
                let kind = classify_consumer_return(&method.sig.output);

                // Responders take a single handler — no fan-out options.
                if matches!(kind, ConsumerKind::Responder { .. })
                    && (config.topic.is_some()
                        || config.deserializer.is_some()
                        || config.filter.is_some()
                        || config.retry.is_some()
                        || config.dlq.is_some())
                {
                    return Err(syn::Error::new_spanned(
                        &method.sig,
                        "a request-reply #[consumer] (non-`()` return) is a responder and does not \
                         support `topic`/`deserializer`/`filter`/`retry`/`dlq` — those are fan-out options",
                    ));
                }

                let mut intercept_fns = impl_intercepts.to_vec();
                intercept_fns.extend(extract_intercept_fns(&method.attrs)?);

                consumers.push(BeanConsumerMethod {
                    config,
                    event_type,
                    kind,
                    fn_name: method.sig.ident.clone(),
                    intercept_fns,
                });
            }
        }
    }

    Ok(consumers)
}

/// Scan the impl block for `#[scheduled(...)]` methods.
fn scan_scheduled_methods(
    item_impl: &ItemImpl,
    impl_intercepts: &[syn::Expr],
) -> syn::Result<Vec<BeanScheduledMethod>> {
    let mut methods = Vec::new();

    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            let Some(config) = extract_scheduled(&method.attrs)? else {
                continue;
            };

            if method.attrs.iter().any(|a| a.path().is_ident("consumer")) {
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    "#[scheduled] and #[consumer] cannot be combined on the same method",
                ));
            }

            let has_self = method
                .sig
                .inputs
                .iter()
                .any(|arg| matches!(arg, FnArg::Receiver(_)));
            if !has_self || method.sig.inputs.len() > 1 {
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    "#[scheduled] methods cannot have parameters other than &self",
                ));
            }

            let mut intercept_fns = impl_intercepts.to_vec();
            intercept_fns.extend(extract_intercept_fns(&method.attrs)?);

            methods.push(BeanScheduledMethod {
                config,
                fn_name: method.sig.ident.clone(),
                is_async: method.sig.asyncness.is_some(),
                intercept_fns,
            });
        }
    }

    Ok(methods)
}

/// Find the constructor method in the impl block.
fn find_constructor(item_impl: &ItemImpl) -> syn::Result<(&syn::ImplItemFn, bool)> {
    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            if method.sig.inputs.iter().any(|arg| matches!(arg, FnArg::Receiver(_))) {
                continue;
            }
            if returns_self(&method.sig.output, &item_impl.self_ty) {
                let is_async = method.sig.asyncness.is_some();
                return Ok((method, is_async));
            }
        }
    }

    Err(syn::Error::new_spanned(
        &item_impl.self_ty,
        "#[bean] requires a constructor — a static method returning Self:\n\
         \n  #[bean]\n  impl MyService {\n      fn new(dep: OtherService) -> Self {\n          Self { dep }\n      }\n  }",
    ))
}

/// Check if a return type is `Self` or matches the impl type.
fn returns_self(ret: &ReturnType, self_ty: &Type) -> bool {
    match ret {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => {
            if let Type::Path(tp) = ty.as_ref() {
                if tp.path.is_ident("Self") {
                    return true;
                }
                if let Type::Path(self_tp) = self_ty {
                    if tp.path.segments.last().map(|s| &s.ident)
                        == self_tp.path.segments.last().map(|s| &s.ident)
                    {
                        return true;
                    }
                }
            }
            false
        }
    }
}

// ── Cleaned impl emission ────────────────────────────────────────────────

/// Rewrites every `Self { .. }` / `<Type> { .. }` struct literal (unless it
/// already carries a `..` rest) to add `__r2e_decos: Default::default()`, so
/// the bean's constructor(s) initialize the hidden slot field injected by the
/// `#[bean]` struct attribute. Runs only when the impl block has intercept
/// sites (the only case that needs the field).
struct DecoLiteralInjector {
    self_last_ident: Option<syn::Ident>,
}

impl DecoLiteralInjector {
    fn matches(&self, path: &syn::Path) -> bool {
        if path.is_ident("Self") {
            return true;
        }
        match (&self.self_last_ident, path.segments.last()) {
            (Some(want), Some(seg)) => &seg.ident == want,
            _ => false,
        }
    }
}

impl VisitMut for DecoLiteralInjector {
    fn visit_expr_struct_mut(&mut self, node: &mut syn::ExprStruct) {
        // Recurse first (nested literals).
        syn::visit_mut::visit_expr_struct_mut(self, node);

        if node.rest.is_some() {
            return; // `..src` fills (and shares) the field — leave it.
        }
        if !self.matches(&node.path) {
            return;
        }
        let already = node.fields.iter().any(|f| match &f.member {
            syn::Member::Named(id) => id == "__r2e_decos",
            _ => false,
        });
        if already {
            return;
        }
        node.fields.push(syn::parse_quote!(__r2e_decos: ::core::default::Default::default()));
    }
}

/// Emit the original impl block with `#[config]`/`#[consumer]`/`#[scheduled]`/
/// `#[post_construct]`/`#[intercept]` attributes stripped, intercepted
/// `#[scheduled]`/`#[consumer]` methods split into inner + dispatch wrapper,
/// `#[async_exec]` methods split into inner + pool-submission wrapper, and
/// (when intercept sites exist) struct literals rewritten to initialize the
/// hidden decorator slot.
fn emit_cleaned_impl(item_impl: &ItemImpl, generated: &GeneratedBean) -> TokenStream2 {
    let intercepted = &generated.intercepted;
    let self_ty = &item_impl.self_ty;
    let has_intercepts = !intercepted.is_empty() || generated.impl_intercepts_present;
    let mut injector = DecoLiteralInjector {
        self_last_ident: match self_ty.as_ref() {
            Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.clone()),
            _ => None,
        },
    };

    let intercepted_by_name = |name: &syn::Ident| intercepted.iter().find(|im| &im.fn_name == name);
    let async_exec_by_name =
        |name: &syn::Ident| generated.async_exec.iter().find(|am| &am.fn_name == name);

    let mut items: Vec<TokenStream2> = Vec::new();

    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            let is_constructor = !method.sig.inputs.iter().any(|arg| matches!(arg, FnArg::Receiver(_)))
                && returns_self(&method.sig.output, self_ty);

            if is_constructor {
                let vis = &method.vis;
                let sig_ident = &method.sig.ident;
                let sig_asyncness = &method.sig.asyncness;
                let sig_output = &method.sig.output;
                let mut body = method.block.clone();
                if has_intercepts {
                    injector.visit_block_mut(&mut body);
                }
                let attrs = &method.attrs;

                let clean_params: Vec<TokenStream2> = method.sig.inputs.iter().map(|arg| {
                    match arg {
                        FnArg::Receiver(r) => quote! { #r },
                        FnArg::Typed(pt) => {
                            let non_config_attrs: Vec<_> = pt.attrs.iter()
                                .filter(|a| {
                                    !a.path().is_ident("config")
                                    && !a.path().is_ident("config_section")
                                    && !a.path().is_ident("inject")
                                })
                                .collect();
                            let pat = &pt.pat;
                            let ty = &pt.ty;
                            quote! { #(#non_config_attrs)* #pat: #ty }
                        }
                    }
                }).collect();

                items.push(quote! {
                    #(#attrs)*
                    #vis #sig_asyncness fn #sig_ident(#(#clean_params),*) #sig_output #body
                });
            } else if let Some(im) = intercepted_by_name(&method.sig.ident) {
                items.push(emit_intercepted_method(method, im, self_ty, &mut injector, has_intercepts));
            } else if let Some(am) = async_exec_by_name(&method.sig.ident) {
                // `#[async_exec]`: split into a renamed inner fn + a synchronous
                // pool-submission wrapper (shared emitter with `#[routes]`).
                let mut cleaned = method.clone();
                cleaned.attrs = strip_async_exec_attrs(cleaned.attrs);
                items.push(transverse::async_exec_method(
                    &cleaned,
                    &am.executor_field,
                    |block| {
                        if has_intercepts {
                            injector.visit_block_mut(block);
                        }
                    },
                ));
            } else {
                // Ordinary method (possibly a non-intercepted scheduled/consumer,
                // or a helper): strip wiring attrs and rewrite any literals.
                let cleaned_attrs: Vec<_> = strip_consumer_attrs(method.attrs.clone())
                    .into_iter()
                    .filter(|a| {
                        !a.path().is_ident("post_construct")
                            && !a.path().is_ident("scheduled")
                            && !a.path().is_ident("intercept")
                    })
                    .collect();
                let vis = &method.vis;
                let sig = &method.sig;
                let mut body = method.block.clone();
                if has_intercepts {
                    injector.visit_block_mut(&mut body);
                }
                items.push(quote! {
                    #(#cleaned_attrs)*
                    #vis #sig #body
                });
            }
        } else {
            items.push(quote! { #item });
        }
    }

    let (impl_generics, _, where_clause) = item_impl.generics.split_for_impl();
    // Strip `#[intercept]` from impl-level attrs (kept as a no-op otherwise).
    let attrs: Vec<_> = item_impl
        .attrs
        .iter()
        .filter(|a| !a.path().is_ident("intercept"))
        .collect();

    quote! {
        #(#attrs)*
        impl #impl_generics #self_ty #where_clause {
            #(#items)*
        }
    }
}

/// Emit an intercepted `#[scheduled]`/`#[consumer]` method as a hidden renamed
/// inner fn + a dispatch wrapper that reads the prebuilt set from the bean's
/// shared decorator slot (via `HasDecoSlot`) and runs the interceptor chain,
/// falling back to a bare inner call when the slot is empty (unregistered
/// bean). A sync scheduled source is promoted to `async fn` (the chain must be
/// awaited).
fn emit_intercepted_method(
    method: &syn::ImplItemFn,
    im: &BeanInterceptedMethod,
    self_ty: &syn::Type,
    injector: &mut DecoLiteralInjector,
    has_intercepts: bool,
) -> TokenStream2 {
    let krate = r2e_core_path();
    let fn_name = &method.sig.ident;
    let params = DispatchWrapperParams {
        container: bean_deco_container_ident(&type_ident(self_ty)),
        field: bean_deco_field_ident(fn_name),
        // The bean reaches its slot through `HasDecoSlot` (a `SharedDecoSlot`).
        slot_access: quote! { <Self as #krate::HasDecoSlot>::__r2e_deco_slot(self) },
        inner_name: format_ident!("__r2e_bean_{}_inner", fn_name),
        owner_name_str: type_ident(self_ty).to_string(),
        source_async: im.source_async,
        event_param: im.event_param.clone(),
        intercept_count: im.intercept_fns.len(),
        origin_macro: "#[bean]",
    };
    // The struct-literal injector only runs when the impl has intercept sites
    // (the only case that materializes the hidden slot field).
    transverse::intercepted_dispatch_wrapper(method, &params, |block| {
        if has_intercepts {
            injector.visit_block_mut(block);
        }
    })
}
