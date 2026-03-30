use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, ReturnType};

use crate::crate_path::r2e_core_path;
use crate::hash_tokens::hash_token_stream;
use crate::type_list_gen::build_tcons_type;
use crate::type_utils::unwrap_option_type;

pub fn expand(input: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(input as ItemFn);
    match generate(&item_fn) {
        Ok(output) => {
            let output = quote! {
                #output
            };
            output.into()
        }
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(item_fn: &ItemFn) -> syn::Result<TokenStream2> {
    let fn_name = &item_fn.sig.ident;
    let is_async = item_fn.sig.asyncness.is_some();

    // Generate PascalCase struct name from fn name (e.g. create_pool -> CreatePool)
    let struct_name = to_pascal_case(&fn_name.to_string());
    let struct_ident = syn::Ident::new(&struct_name, fn_name.span());

    // Extract the return type as the Output type
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

    // Process parameters — detect #[config("key")] vs regular dependencies
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
                    let key: syn::LitStr = attr.parse_args()?;
                    let key_str = key.value();
                    let env_hint = key_str.replace('.', "_").to_uppercase();
                    let ty_name_str = quote!(#ty).to_string();
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
                } else if let Some(inner_ty) = unwrap_option_type(ty) {
                    build_args.push(quote! { let #arg_name: #ty = ctx.try_get::<#inner_ty>(); });
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

    Ok(quote! {
        // Emit the original function with cleaned params
        #vis #fn_asyncness fn #fn_name(#(#clean_params),*) #ret_ty #fn_body

        // Generated producer struct
        #vis struct #struct_ident;

        impl #krate::beans::Producer for #struct_ident {
            type Output = #output_ty;
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
                #call
            }
        }
    })
}

/// Parse `#[config_section(prefix = "...")]` and return the prefix string.
fn parse_config_section_prefix(attr: &syn::Attribute) -> syn::Result<String> {
    let mut prefix: Option<String> = None;
    if let syn::Meta::List(_) = &attr.meta {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("prefix") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                prefix = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error("expected `prefix` in #[config_section(prefix = \"...\")]"))
            }
        })?;
    }
    prefix.ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "#[config_section] requires a prefix: #[config_section(prefix = \"app\")]",
        )
    })
}

/// Convert a snake_case name to PascalCase.
fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}
