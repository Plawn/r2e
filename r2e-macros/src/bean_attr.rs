use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ImplItem, ItemImpl, ReturnType, Type};

use crate::crate_path::{r2e_core_path, r2e_events_path};
use crate::extract::consumer::{extract_consumer, extract_event_type_from_arc, strip_consumer_attrs};
use crate::hash_tokens::hash_token_stream;
use crate::type_list_gen::build_tcons_type;
use crate::type_utils::{unwrap_option_type, parse_inject_name, named_bean_newtype_ident, parse_config_section_prefix};

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
    fn_name: syn::Ident,
}

/// Parsed post-construct method data from a `#[bean]` impl block.
struct BeanPostConstructMethod {
    fn_name: syn::Ident,
    is_async: bool,
    returns_result: bool,
}

pub fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let bean_args = match BeanArgs::parse(args) {
        Ok(a) => a,
        Err(err) => return err.to_compile_error().into(),
    };
    let item_impl = parse_macro_input!(input as ItemImpl);
    match generate(&item_impl, &bean_args) {
        Ok(bean_impl) => {
            // Emit the original impl with #[config] and #[consumer] attrs stripped
            let cleaned_impl = strip_attrs_from_impl(&item_impl);
            let output = quote! {
                #cleaned_impl
                #bean_impl
            };
            output.into()
        }
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(item_impl: &ItemImpl, bean_args: &BeanArgs) -> syn::Result<TokenStream2> {
    // Extract the Self type from the impl block.
    let self_ty = &item_impl.self_ty;

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

                // Check for #[inject(name = "...")] attribute
                let inject_name = parse_inject_name(&pat_type.attrs)?;

                // Check for #[config("key")] or #[config_section(prefix = "...")] attribute
                let config_attr = pat_type.attrs.iter().find(|a| a.path().is_ident("config"));
                let config_section_attr = pat_type.attrs.iter().find(|a| a.path().is_ident("config_section"));

                if let Some(name) = inject_name {
                    // Named injection: resolve via generated newtype
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
                    let key: syn::LitStr = attr.parse_args()?;
                    let key_str = key.value();
                    let env_hint = key_str.replace('.', "_").to_uppercase();
                    let ty_name_str = quote!(#ty).to_string();
                    config_key_entries.push(quote! { (#key_str, #ty_name_str) });
                    build_args.push(quote! {
                        let #arg_name: #ty = __r2e_config.get::<#ty>(#key_str).unwrap_or_else(|_| {
                            panic!(
                                "Configuration error in bean `{}`: key '{}' — Config key not found. \
                                 Add it to application.yaml or set env var `{}`.",
                                #type_name_str, #key_str, #env_hint
                            )
                        });
                    });
                    has_config = true;
                } else if let Some(inner_ty) = unwrap_option_type(ty) {
                    build_args.push(quote! { let #arg_name: #ty = ctx.try_get::<#inner_ty>(); });
                } else {
                    dep_type_ids.push(quote! { (std::any::TypeId::of::<#ty>(), std::any::type_name::<#ty>()) });
                    dep_types.push(quote! { #ty });
                    build_args.push(quote! { let #arg_name: #ty = ctx.get::<#ty>(); });
                }
            }
        }
    }

    // If any #[config] params, add R2eConfig to the dependency list once
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
    let deps_type = build_tcons_type(&dep_types, &krate);

    // Compute BUILD_VERSION from the constructor body tokens
    let build_version = hash_token_stream(&quote! { #constructor });

    // Extract R2eConfig once if any #[config] params are present
    let config_prelude = if has_config {
        quote! { let __r2e_config: #krate::config::R2eConfig = ctx.get::<#krate::config::R2eConfig>(); }
    } else {
        quote! {}
    };

    let config_keys_fn = if config_key_entries.is_empty() {
        quote! {}
    } else {
        quote! {
            fn config_keys() -> Vec<(&'static str, &'static str)> {
                vec![#(#config_key_entries),*]
            }
        }
    };

    // Scan for #[consumer] methods
    let consumer_methods = scan_consumer_methods(item_impl)?;
    if bean_args.lazy && !consumer_methods.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_impl.self_ty,
            "#[bean(lazy)] does not yet support #[consumer] methods — \
             remove one or the other",
        ));
    }
    let subscriber_impl = generate_event_subscriber_impl(self_ty, &consumer_methods)?;

    // Scan for #[post_construct] methods
    let pc_methods = scan_post_construct_methods(item_impl)?;
    if bean_args.lazy && !pc_methods.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_impl.self_ty,
            "#[bean(lazy)] does not yet support #[post_construct] — \
             remove one or the other",
        ));
    }
    let post_construct_impl = generate_post_construct_impl(self_ty, &pc_methods);
    let after_register_fn = if !pc_methods.is_empty() {
        quote! {
            fn after_register(registry: &mut #krate::beans::BeanRegistry) {
                registry.register_post_construct::<Self>();
            }
        }
    } else {
        quote! {}
    };

    let lazy_const = bean_args.lazy;

    let bean_impl = if is_async {
        // Generate AsyncBean impl
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
        // Generate Bean impl
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

    Ok(quote! {
        #bean_impl
        #post_construct_impl
        #subscriber_impl
    })
}

/// Scan all `&self` methods in the impl block for `#[consumer(bus = "...")]` attributes.
fn scan_consumer_methods(item_impl: &ItemImpl) -> syn::Result<Vec<BeanConsumerMethod>> {
    let mut consumers = Vec::new();

    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            // Only consider methods with &self receiver
            let has_self = method
                .sig
                .inputs
                .iter()
                .any(|arg| matches!(arg, FnArg::Receiver(_)));
            if !has_self {
                continue;
            }

            if let Some(config) = extract_consumer(&method.attrs)? {
                // Find the event parameter (first typed param after &self)
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

                consumers.push(BeanConsumerMethod {
                    config,
                    event_type,
                    fn_name: method.sig.ident.clone(),
                });
            }
        }
    }

    Ok(consumers)
}

/// Generate `EventSubscriber` impl if any consumer methods are found.
fn generate_event_subscriber_impl(
    self_ty: &syn::Type,
    consumers: &[BeanConsumerMethod],
) -> syn::Result<TokenStream2> {
    if consumers.is_empty() {
        return Ok(quote! {});
    }

    let krate = r2e_core_path();
    let events_krate = r2e_events_path();

    let subscribe_blocks: Vec<_> = consumers
        .iter()
        .map(|cm| {
            let bus_field = syn::Ident::new(&cm.config.bus_field, proc_macro2::Span::call_site());
            let event_type = &cm.event_type;
            let fn_name = &cm.fn_name;

            // Optional: register topic before subscribe
            let register_topic = cm.config.topic.as_ref().map(|topic_str| {
                quote! {
                    #events_krate::EventBus::register_topic::<#event_type>(&__bus, #topic_str).await;
                }
            });

            // Choose subscribe vs subscribe_with_deserializer
            let subscribe_call = if let Some(ref deser_fn) = cm.config.deserializer {
                let deser_ident = syn::Ident::new(deser_fn, proc_macro2::Span::call_site());
                quote! {
                    let __deser: #events_krate::backend::DeserializerFn = std::sync::Arc::new(Self::#deser_ident);
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

            // Build optional filter
            let has_filter = cm.config.filter.is_some();
            let filter_expr = if let Some(ref filter_fn) = cm.config.filter {
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

            // Build optional retry policy
            let has_retry = cm.config.retry.is_some();
            let retry_expr = if let Some(max_retries) = cm.config.retry {
                if let Some(ref dlq_topic) = cm.config.dlq {
                    quote! {
                        Some(#events_krate::RetryPolicy::new(#max_retries).with_dlq(#dlq_topic))
                    }
                } else {
                    quote! {
                        Some(#events_krate::RetryPolicy::new(#max_retries))
                    }
                }
            } else {
                quote! { None }
            };

            // Generate configure_handler call if needed
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
                    let __bus = self.#bus_field.clone();
                    let __bus_ref = __bus.clone();
                    let __this_orig = self.clone();
                    let __this = self.clone();
                    #register_topic
                    let __handle = #subscribe_call;
                    #configure_handler
                    if let Err(__e) = __handle {
                        eprintln!("[r2e] Failed to subscribe consumer: {__e}");
                    }
                }
            }
        })
        .collect();

    Ok(quote! {
        impl #krate::EventSubscriber for #self_ty {
            fn subscribe(self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
                Box::pin(async move {
                    #(#subscribe_blocks)*
                })
            }
        }
    })
}

/// Scan all `&self` methods in the impl block for `#[post_construct]` attributes.
fn scan_post_construct_methods(item_impl: &ItemImpl) -> syn::Result<Vec<BeanPostConstructMethod>> {
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

            // Validate: no extra params beyond &self
            let param_count = method.sig.inputs.len();
            if param_count > 1 {
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    "#[post_construct] method must take only `&self` — no additional parameters",
                ));
            }

            let is_async = method.sig.asyncness.is_some();

            // Check if return type is Result<(), ...>
            let returns_result = match &method.sig.output {
                ReturnType::Default => false,
                ReturnType::Type(_, ty) => {
                    let ty_str = quote!(#ty).to_string();
                    ty_str.starts_with("Result")
                }
            };

            methods.push(BeanPostConstructMethod {
                fn_name: method.sig.ident.clone(),
                is_async,
                returns_result,
            });
        }
    }

    Ok(methods)
}

/// Generate `PostConstruct` impl if any post-construct methods are found.
fn generate_post_construct_impl(
    self_ty: &syn::Type,
    methods: &[BeanPostConstructMethod],
) -> TokenStream2 {
    if methods.is_empty() {
        return quote! {};
    }

    let krate = r2e_core_path();

    let calls: Vec<TokenStream2> = methods
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
        impl #krate::beans::PostConstruct for #self_ty {
            fn post_construct(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + '_>> {
                Box::pin(async move {
                    #(#calls)*
                    Ok(())
                })
            }
        }
    }
}

/// Find the constructor method in the impl block.
///
/// The constructor is the first associated function (no `self` receiver)
/// whose return type is `Self` or matches the impl type name.
/// Returns the method and whether it is async.
fn find_constructor(item_impl: &ItemImpl) -> syn::Result<(&syn::ImplItemFn, bool)> {
    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            // Skip methods with a self receiver.
            if method.sig.inputs.iter().any(|arg| matches!(arg, FnArg::Receiver(_))) {
                continue;
            }

            // Check return type is Self or the type name.
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
            // Check for `-> Self`
            if let Type::Path(tp) = ty.as_ref() {
                if tp.path.is_ident("Self") {
                    return true;
                }
                // Check if it matches the type name (e.g., `-> UserService`)
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


/// Strip `#[config(...)]`, `#[config_section(...)]`, and `#[inject(...)]` attributes from
/// the constructor parameters and `#[consumer(...)]` attributes from methods in the emitted impl block.
fn strip_attrs_from_impl(item_impl: &ItemImpl) -> TokenStream2 {
    let mut items: Vec<TokenStream2> = Vec::new();

    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            // Check if this is the constructor (no self, returns Self)
            let is_constructor = !method.sig.inputs.iter().any(|arg| matches!(arg, FnArg::Receiver(_)))
                && returns_self(&method.sig.output, &item_impl.self_ty);

            if is_constructor {
                // Rebuild the function with #[config] attrs stripped from params
                let vis = &method.vis;
                let sig_ident = &method.sig.ident;
                let sig_asyncness = &method.sig.asyncness;
                let sig_output = &method.sig.output;
                let body = &method.block;
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
            } else {
                // Strip #[consumer] and #[post_construct] attrs from methods
                let cleaned_attrs: Vec<_> = strip_consumer_attrs(method.attrs.clone())
                    .into_iter()
                    .filter(|a| !a.path().is_ident("post_construct"))
                    .collect();
                let vis = &method.vis;
                let sig = &method.sig;
                let body = &method.block;
                items.push(quote! {
                    #(#cleaned_attrs)*
                    #vis #sig #body
                });
            }
        } else {
            items.push(quote! { #item });
        }
    }

    let self_ty = &item_impl.self_ty;
    let (impl_generics, _, where_clause) = item_impl.generics.split_for_impl();
    let attrs = &item_impl.attrs;

    quote! {
        #(#attrs)*
        impl #impl_generics #self_ty #where_clause {
            #(#items)*
        }
    }
}
