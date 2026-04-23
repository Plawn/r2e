use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, ReturnType};

use crate::crate_path::r2e_core_path;
use crate::hash_tokens::hash_token_stream;
use crate::type_list_gen::build_tcons_type;
use crate::type_utils::{parse_config_field, parse_config_section_prefix, to_pascal_case, type_base_name};

/// Parsed `#[producer(...)]` arguments.
struct ProducerArgs {
    /// If present, the `name = "..."` qualifier for named beans.
    name: Option<String>,
}

impl ProducerArgs {
    fn parse(args: TokenStream) -> syn::Result<Self> {
        let mut name = None;
        if !args.is_empty() {
            let parser = syn::meta::parser(|meta| {
                if meta.path.is_ident("name") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    name = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `name = \"...\"`"))
                }
            });
            syn::parse::Parser::parse(parser, args)?;
        }
        Ok(Self { name })
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
    let output_ty = match &item_fn.sig.output {
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                fn_name,
                "#[producer] function must have a return type:\n\
                 \n  #[producer]\n  async fn create_pool() -> SqlitePool { ... }",
            ));
        }
        ReturnType::Type(_, ty) => ty.as_ref().clone(),
    };

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

    // Collect parameter info, stripping #[config] attrs
    let mut clean_params: Vec<TokenStream2> = Vec::new();

    for (i, arg) in item_fn.sig.inputs.iter().enumerate() {
        match arg {
            FnArg::Receiver(_) => unreachable!(), // checked above
            FnArg::Typed(pat_type) => {
                let ty = &*pat_type.ty;
                let arg_name =
                    syn::Ident::new(&format!("__arg_{}", i), proc_macro2::Span::call_site());

                // Check for #[config("key")] or #[config_section(prefix = "...")] attribute
                let config_attr = pat_type
                    .attrs
                    .iter()
                    .find(|a| a.path().is_ident("config"));
                let config_section_attr = pat_type
                    .attrs
                    .iter()
                    .find(|a| a.path().is_ident("config_section"));

                if let Some(attr) = config_section_attr {
                    let prefix_str = parse_config_section_prefix(attr)?;
                    let krate = r2e_core_path();
                    build_args.push(quote! {
                        let #arg_name: #ty = #krate::config::ConfigProperties::from_config(&__r2e_config, Some(#prefix_str)).unwrap_or_else(|e| {
                            panic!(
                                "Configuration error in producer `{}`: config section '{}' — {}",
                                #struct_name, #prefix_str, e
                            )
                        });
                    });
                    has_config = true;
                } else if let Some(attr) = config_attr {
                    let (key_str, env_hint, ty_name_str) = parse_config_field(attr, ty)?;
                    config_key_entries.push(quote! { (#key_str, #ty_name_str) });
                    build_args.push(quote! {
                        let #arg_name: #ty = __r2e_config.get::<#ty>(#key_str).unwrap_or_else(|_| {
                            panic!(
                                "Configuration error in producer `{}`: key '{}' — Config key not found. \
                                 Add it to application.yaml or set env var `{}`.",
                                #struct_name, #key_str, #env_hint
                            )
                        });
                    });
                    has_config = true;
                } else {
                    dep_type_ids
                        .push(quote! { (std::any::TypeId::of::<#ty>(), std::any::type_name::<#ty>()) });
                    dep_types.push(quote! { #ty });
                    build_args.push(quote! { let #arg_name: #ty = ctx.get::<#ty>(); });
                }

                // Build clean param (without #[config] attr)
                let pat = &pat_type.pat;
                let non_config_attrs: Vec<_> = pat_type
                    .attrs
                    .iter()
                    .filter(|a| !a.path().is_ident("config") && !a.path().is_ident("config_section"))
                    .collect();
                clean_params.push(quote! { #(#non_config_attrs)* #pat: #ty });
            }
        }
    }

    // If any #[config] params, add R2eConfig to dependencies
    if has_config {
        let krate = r2e_core_path();
        dep_type_ids.push(
            quote! { (std::any::TypeId::of::<#krate::config::R2eConfig>(), std::any::type_name::<#krate::config::R2eConfig>()) },
        );
        dep_types.push(quote! { #krate::config::R2eConfig });
    }

    let arg_forwards: Vec<_> = (0..item_fn.sig.inputs.len())
        .map(|i| {
            let arg_name =
                syn::Ident::new(&format!("__arg_{}", i), proc_macro2::Span::call_site());
            quote! { #arg_name }
        })
        .collect();

    let krate = r2e_core_path();
    let deps_type = build_tcons_type(&dep_types, &krate);

    // Compute BUILD_VERSION from the function body tokens
    let build_version = hash_token_stream(&quote! { #item_fn });

    // Extract R2eConfig once if any #[config] params are present
    let config_prelude = if has_config {
        quote! { let __r2e_config: #krate::config::R2eConfig = ctx.get::<#krate::config::R2eConfig>(); }
    } else {
        quote! {}
    };

    // Generate the call to the original function
    let call = if is_async {
        quote! { #fn_name(#(#arg_forwards),*).await }
    } else {
        quote! { #fn_name(#(#arg_forwards),*) }
    };

    // Emit the original function (with #[config] stripped from params) + the producer struct + impl
    let vis = &item_fn.vis;
    let fn_body = &item_fn.block;
    let fn_asyncness = &item_fn.sig.asyncness;
    let ret_ty = &item_fn.sig.output;

    // If `name = "..."` is specified, generate a newtype wrapper around the
    // output type. The newtype is what gets registered in the bean context,
    // and consumers using `#[inject(name = "...")] field: T` resolve via
    // the newtype.
    //
    // Note: named producers returning `Option<T>` are not supported — named
    // resolution goes through the newtype wrapper, which doesn't compose with
    // `Option<T>`. Use an unnamed producer for the conditional-availability
    // pattern.
    let (effective_output_ty, newtype_decl, produce_expr) = if let Some(ref name) = args.name {
        let newtype_name = format!("{}{}", to_pascal_case(name), type_base_name(&output_ty));
        let newtype_ident = syn::Ident::new(&newtype_name, fn_name.span());
        let doc = format!("Generated newtype for named bean `{}`.", name);
        let newtype = quote! {
            #[doc = #doc]
            #[derive(Clone)]
            #vis struct #newtype_ident(pub #output_ty);

            impl ::std::ops::Deref for #newtype_ident {
                type Target = #output_ty;
                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }
        };
        let wrapped_ty: TokenStream2 = quote! { #newtype_ident };
        let expr = quote! { #newtype_ident(#call) };
        (wrapped_ty, newtype, expr)
    } else {
        (quote! { #output_ty }, quote! {}, quote! { #call })
    };

    Ok(quote! {
        // Emit the original function with cleaned params
        #vis #fn_asyncness fn #fn_name(#(#clean_params),*) #ret_ty #fn_body

        // Generated newtype (if named)
        #newtype_decl

        // Generated producer struct
        #vis struct #struct_ident;

        impl #krate::beans::Producer for #struct_ident {
            type Output = #effective_output_ty;
            type Deps = #deps_type;

            fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                vec![#(#dep_type_ids),*]
            }

            fn config_keys() -> Vec<(&'static str, &'static str)> {
                vec![#(#config_key_entries),*]
            }

            const BUILD_VERSION: u64 = #build_version;

            async fn produce(ctx: &#krate::beans::BeanContext) -> Self::Output {
                #config_prelude
                #(#build_args)*
                #produce_expr
            }
        }
    })
}
