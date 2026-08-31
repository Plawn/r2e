use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, ReturnType};

use crate::model::type_list_gen::build_tcons_type;
use crate::util::crate_path::r2e_core_path;
use crate::util::hash_tokens::hash_token_stream;
use crate::util::type_utils::{
    check_bean_inject_args, parse_config_field, parse_config_section_prefix,
    parse_live_config_field, result_ok_err_types, to_pascal_case, NAMED_BEAN_MSG,
};

/// Parsed `#[producer(...)]` arguments.
struct ProducerArgs {
    /// Whether the produced output should be started as a lifecycle service.
    start: bool,
    /// `after(A, B)` — ordering-only dependency edges.
    ///
    /// Each type joins `Producer::Deps` and `Producer::dependencies()` without
    /// becoming a function parameter: the graph builds it first (and
    /// compile-checks that it is registered), but the producer body never sees
    /// it. The alternative people reach for today is an unused parameter
    /// (`_guard: InstanceGuard`), which reads as a mistake and trips
    /// `unused_variables` lints.
    after: Vec<syn::Type>,
}

impl ProducerArgs {
    fn parse(args: TokenStream) -> syn::Result<Self> {
        let mut start = false;
        let mut after: Vec<syn::Type> = Vec::new();
        if !args.is_empty() {
            let parser = syn::meta::parser(|meta| {
                if meta.path.is_ident("start") {
                    start = true;
                    Ok(())
                } else if meta.path.is_ident("after") {
                    let content;
                    syn::parenthesized!(content in meta.input);
                    let types = content.parse_terminated(
                        <syn::Type as syn::parse::Parse>::parse,
                        syn::Token![,],
                    )?;
                    if types.is_empty() {
                        return Err(meta.error(
                            "#[producer(after(..))] needs at least one type: \
                             #[producer(after(InstanceGuard))]",
                        ));
                    }
                    after.extend(types);
                    Ok(())
                } else if meta.path.is_ident("name") {
                    Err(meta.error(NAMED_BEAN_MSG))
                } else {
                    Err(meta.error("expected `start` or `after(Type, ..)`"))
                }
            });
            syn::parse::Parser::parse(parser, args)?;
        }
        Ok(Self { start, after })
    }
}

pub fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let producer_args = match ProducerArgs::parse(args) {
        Ok(a) => a,
        Err(err) => return err.to_compile_error().into(),
    };
    let item_fn = parse_macro_input!(input as ItemFn);
    match generate(&item_fn, &producer_args) {
        Ok(output) => {
            let output = quote! {
                #output
            };
            output.into()
        }
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(item_fn: &ItemFn, args: &ProducerArgs) -> syn::Result<TokenStream2> {
    let fn_name = &item_fn.sig.ident;
    let is_async = item_fn.sig.asyncness.is_some();
    crate::util::type_utils::reject_unsafe_constructor(
        &item_fn.sig,
        "#[producer]",
        "Producer::produce",
    )?;

    // Generate PascalCase struct name from fn name (e.g. create_pool -> CreatePool)
    let struct_name = to_pascal_case(&fn_name.to_string());
    let struct_ident = syn::Ident::new(&struct_name, fn_name.span());

    // Extract the return type as the Output type.
    //
    // The return type is registered verbatim — if the user returns `Option<T>`,
    // the bean is registered under `Option<T>` (the whole type, not the inner
    // `T`). Consumers inject `Option<T>` as a hard dependency. This lets
    // `#[producer]` express conditional availability without a separate
    // "soft dependency" mechanism.
    //
    // The one exception is a literal `Result<T, E>`: it is split into
    // `Producer::Output = T` + `Producer::Error = E`, so the *success* type is
    // the bean and the error aborts `build_state()` naming the bean. A producer
    // that genuinely wants to register a `Result` as a value returns a newtype.
    let declared_ty = match &item_fn.sig.output {
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                fn_name,
                "#[producer] function must have a return type:\n\
                 \n  #[producer]\n  async fn create_pool() -> SqlitePool { ... }",
            ));
        }
        ReturnType::Type(_, ty) => ty.as_ref().clone(),
    };

    // `anyhow::Result<Pool>` and friends would fall through the split below
    // into the infallible arm, registering the bean under the *Result* type.
    // Refuse the declaration instead of misclassifying it.
    crate::util::type_utils::reject_single_arg_result_alias(&declared_ty, "#[producer]")?;

    let (output_ty, error_ty) = match result_ok_err_types(&declared_ty) {
        Some((ok, err)) => (ok.clone(), quote! { #err }),
        None => (declared_ty.clone(), quote! { ::std::convert::Infallible }),
    };
    let is_fallible = result_ok_err_types(&declared_ty).is_some();

    // Check no self parameter
    if item_fn
        .sig
        .inputs
        .iter()
        .any(|arg| matches!(arg, FnArg::Receiver(_)))
    {
        return Err(syn::Error::new_spanned(
            fn_name,
            "#[producer] must be a free function (no `self` parameter):\n\
             \n  #[producer]\n  async fn create_pool(#[config(\"app.db.url\")] url: String) -> SqlitePool { ... }",
        ));
    }

    // Process parameters — detect #[config("key")] vs regular dependencies.
    //
    // Note: `Option<T>` parameters are treated as hard dependencies on the
    // whole `Option<T>` type (not the inner `T`). A producer must register
    // `Option<T>` in the context for such a parameter to resolve.
    let mut dep_type_ids = Vec::new();
    let mut dep_types: Vec<TokenStream2> = Vec::new();
    let mut build_args = Vec::new();
    let mut config_key_entries = Vec::new();
    let mut has_config = false;
    let mut has_live_config = false;

    // Collect parameter info, stripping #[config] attrs
    let mut clean_params: Vec<TokenStream2> = Vec::new();

    for (i, arg) in item_fn.sig.inputs.iter().enumerate() {
        match arg {
            FnArg::Receiver(_) => unreachable!(), // checked above
            FnArg::Typed(pat_type) => {
                let ty = &*pat_type.ty;
                let arg_name =
                    syn::Ident::new(&format!("__arg_{}", i), proc_macro2::Span::call_site());

                check_bean_inject_args(&pat_type.attrs)?;

                // Check for #[config("key")] or #[config_section(prefix = "...")] attribute
                let config_attr = pat_type.attrs.iter().find(|a| a.path().is_ident("config"));
                let config_section_attr = pat_type
                    .attrs
                    .iter()
                    .find(|a| a.path().is_ident("config_section"));
                let live_config_attr = pat_type
                    .attrs
                    .iter()
                    .find(|a| a.path().is_ident("live_config"));

                crate::model::field_resolver::check_live_config_exclusive(&pat_type.attrs)?;

                if let Some(attr) = config_section_attr {
                    let prefix_str = parse_config_section_prefix(attr)?;
                    let krate = r2e_core_path();
                    // Declared as `Section`: the key is the PREFIX, so
                    // dev-reload fingerprints the whole subtree under it.
                    config_key_entries.push(
                        crate::model::field_resolver::section_config_key_entry(
                            &krate,
                            &prefix_str,
                            ty,
                        ),
                    );
                    let owner = format!("producer `{struct_name}`");
                    let expr = crate::model::field_resolver::config_section_resolve_expr(
                        &quote! { __r2e_config },
                        &prefix_str,
                        ty,
                        &krate,
                        &owner,
                    );
                    build_args.push(quote! { let #arg_name: #ty = #expr; });
                    has_config = true;
                } else if let Some(attr) = config_attr {
                    let (key_str, ty_name_str) = parse_config_field(attr, ty)?;
                    let krate = r2e_core_path();
                    let is_option = crate::util::type_utils::is_option_type(ty);
                    // Emit a `config_keys()` entry for EVERY copied key
                    // (required and optional) so dev-reload fingerprints the
                    // value; the kind gates presence validation.
                    config_key_entries.push(crate::model::field_resolver::copied_config_key_entry(
                        &krate,
                        &key_str,
                        &ty_name_str,
                        is_option,
                    ));
                    let owner = format!("producer `{struct_name}`");
                    let expr = crate::model::field_resolver::config_resolve_expr(
                        &quote! { __r2e_config },
                        &key_str,
                        Some(ty),
                        &owner,
                        is_option,
                        &krate,
                    );
                    build_args.push(quote! { let #arg_name: #ty = #expr; });
                    has_config = true;
                } else if let Some(attr) = live_config_attr {
                    let (key_str, ty_name_str) = parse_live_config_field(attr, ty)?;
                    // Declared as `Live`: never presence-validated and never
                    // fingerprinted — the value is pushed through the registry
                    // slot, so editing it must NOT rebuild the producer.
                    config_key_entries.push(crate::model::field_resolver::live_config_key_entry(
                        &r2e_core_path(),
                        &key_str,
                        &ty_name_str,
                    ));
                    let expr = crate::model::field_resolver::live_config_resolve_expr(
                        &quote! { __r2e_live },
                        &key_str,
                        Some(ty),
                    );
                    build_args.push(quote! { let #arg_name: #ty = #expr; });
                    has_live_config = true;
                } else {
                    dep_type_ids.push(
                        quote! { (std::any::TypeId::of::<#ty>(), std::any::type_name::<#ty>()) },
                    );
                    dep_types.push(quote! { #ty });
                    build_args.push(quote! { let #arg_name: #ty = ctx.get::<#ty>(); });
                }

                // Build clean param (without #[config] attr)
                let pat = &pat_type.pat;
                let non_config_attrs: Vec<_> = pat_type
                    .attrs
                    .iter()
                    .filter(|a| {
                        !a.path().is_ident("config")
                            && !a.path().is_ident("config_section")
                            && !a.path().is_ident("live_config")
                    })
                    .collect();
                clean_params.push(quote! { #(#non_config_attrs)* #pat: #ty });
            }
        }
    }

    // `after(A, B)`: ordering-only edges. They join `dependencies()` (the
    // topological order the graph resolves in) and `Deps` (the compile-time
    // presence check), but produce no `build_args` / `arg_forwards` entry, so
    // the producer function keeps its written signature.
    for ty in &args.after {
        if dep_types
            .iter()
            .any(|d| d.to_string() == quote!(#ty).to_string())
        {
            return Err(syn::Error::new_spanned(
                ty,
                "`after(..)` names a type this producer already takes as a parameter — \
                 a parameter is already a dependency edge; drop it from `after(..)`",
            ));
        }
        dep_type_ids
            .push(quote! { (std::any::TypeId::of::<#ty>(), std::any::type_name::<#ty>()) });
        dep_types.push(quote! { #ty });
    }

    // If any #[config] params, add R2eConfig to dependencies
    if has_config {
        let krate = r2e_core_path();
        dep_type_ids.push(
            quote! { (std::any::TypeId::of::<#krate::config::R2eConfig>(), std::any::type_name::<#krate::config::R2eConfig>()) },
        );
        dep_types.push(quote! { #krate::config::R2eConfig });
    }
    if has_live_config {
        let krate = r2e_core_path();
        let live_ty = crate::model::field_resolver::live_config_registry_ty(&krate);
        dep_type_ids.push(
            quote! { (std::any::TypeId::of::<#live_ty>(), std::any::type_name::<#live_ty>()) },
        );
        dep_types.push(live_ty);
    }

    let arg_forwards: Vec<_> = (0..item_fn.sig.inputs.len())
        .map(|i| {
            let arg_name = syn::Ident::new(&format!("__arg_{}", i), proc_macro2::Span::call_site());
            quote! { #arg_name }
        })
        .collect();

    let krate = r2e_core_path();
    let param_deps_type = build_tcons_type(&dep_types, &krate);
    let after_register_fn = if args.start {
        quote! {
            fn after_register(registry: &mut #krate::beans::BeanRegistry) {
                registry.register_service_source::<Self::Output>();
            }
        }
    } else {
        quote! {}
    };

    // Compute BUILD_VERSION from the function body tokens
    let build_version = hash_token_stream(&quote! { #item_fn });

    // Extract R2eConfig once if any #[config] params are present
    let config_prelude = if has_config {
        quote! { let __r2e_config: #krate::config::R2eConfig = ctx.get::<#krate::config::R2eConfig>(); }
    } else {
        quote! {}
    };
    let live_config_prelude =
        crate::model::field_resolver::live_config_prelude(&quote! { ctx }, &krate, has_live_config);

    // Generate the call to the original function
    let call = if is_async {
        quote! { #fn_name(#(#arg_forwards),*).await }
    } else {
        quote! { #fn_name(#(#arg_forwards),*) }
    };

    // Emit the original function (with #[config] stripped from params) + the producer struct + impl
    let vis = &item_fn.vis;
    let fn_body = &item_fn.block;
    let fn_constness = &item_fn.sig.constness;
    let fn_asyncness = &item_fn.sig.asyncness;
    let fn_abi = &item_fn.sig.abi;
    let fn_generics = &item_fn.sig.generics;
    let fn_where = &item_fn.sig.generics.where_clause;
    let ret_ty = &item_fn.sig.output;

    // The user's own attributes travel with the re-emitted function. Rebuilding
    // the signature from pieces used to drop them wholesale, so `#[allow]`,
    // `#[inline]`, `#[deprecated]` and doc comments written on a `#[producer]`
    // silently did nothing (task #985).
    let fn_attrs = &item_fn.attrs;

    // `#[cfg]` needs no special handling here — and must NOT be copied onto the
    // generated items. rustc evaluates item-level `#[cfg]` *before* it invokes
    // an attribute macro (whichever order the two are written in), so a
    // cfg'd-out producer never reaches this code and a cfg'd-in one arrives
    // with the attribute already stripped. `r2e-core/tests/di/producer_attrs.rs`
    // guards both orders.

    // A producer takes one parameter per dependency, so `too_many_arguments`
    // fires on perfectly idiomatic producers. Allow it by default; the user's
    // own attributes are emitted AFTER ours, so an explicit
    // `#[warn(clippy::too_many_arguments)]` still wins.
    let allow_many_args = quote! { #[allow(clippy::too_many_arguments)] };

    // The generated struct is user-visible (it carries the function's
    // visibility), so give it its own doc: a crate with `#![deny(missing_docs)]`
    // must not fail because of an item the macro synthesised.
    let struct_doc = format!(
        "Producer bean generated by `#[producer]` from the `{fn_name}` function.\n\n\
         Register it on the builder with `.register::<{struct_name}>()`."
    );

    // The produced type IS the bean key: R2E has no qualifiers, so a producer
    // that needs to coexist with another of the same underlying type returns a
    // newtype the user declares (`struct ReadPool(PgPool)`).
    let effective_output_ty: TokenStream2 = quote! { #output_ty };
    let produce_expr = if is_fallible {
        quote! { #call }
    } else {
        quote! { ::std::result::Result::<_, ::std::convert::Infallible>::Ok(#call) }
    };

    let config_keys_ret_ty = crate::model::field_resolver::config_keys_ret_ty(&krate);

    // `#[producer(start)]` registers `Self::Output` as a background service, so
    // the graph must also satisfy what the *service* pulls in
    // `ServiceComponent::from_context` — not just what the producer function
    // takes as parameters. Without this fold, a derived service with a missing
    // `#[inject]` bean compiled fine and panicked in `ctx.get()` at serve time.
    //
    // The output type is registered verbatim (bare `T` or `Option<T>`), and
    // `register_service_source::<Self::Output>()`
    // already requires exactly that type to be the `ServiceComponent`, so the
    // same type is the one to ask for `Deps`. Both lists are concrete, so the
    // `TAppend` projection normalizes without extra bounds.
    let deps_type = if args.start {
        quote! {
            <#param_deps_type as #krate::type_list::TAppend<
                <#effective_output_ty as #krate::ServiceComponent>::Deps,
            >>::Output
        }
    } else {
        param_deps_type
    };

    Ok(quote! {
        // Emit the original function with cleaned params, keeping every
        // attribute the user wrote on it (doc comments included).
        #allow_many_args
        #(#fn_attrs)*
        #vis #fn_constness #fn_asyncness #fn_abi fn #fn_name #fn_generics (#(#clean_params),*) #ret_ty
        #fn_where
        #fn_body

        // Generated producer struct
        #[doc = #struct_doc]
        #vis struct #struct_ident;

        #allow_many_args
        #[allow(deprecated)]
        impl #krate::beans::Producer for #struct_ident {
            type Output = #effective_output_ty;
            type Error = #error_ty;
            type Deps = #deps_type;

            fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                vec![#(#dep_type_ids),*]
            }

            fn config_keys() -> #config_keys_ret_ty {
                vec![#(#config_key_entries),*]
            }

            const BUILD_VERSION: u64 = #build_version;

            async fn produce(
                ctx: &#krate::beans::BeanContext,
            ) -> ::std::result::Result<Self::Output, Self::Error> {
                #config_prelude
                #live_config_prelude
                #(#build_args)*
                #produce_expr
            }

            #after_register_fn
        }

        impl #krate::beans::Registrable for #struct_ident {
            type Provided = #effective_output_ty;
            type Deps = #deps_type;

            fn register_into(registry: &mut #krate::beans::BeanRegistry) {
                registry.register_producer::<Self>();
            }
        }
    })
}
