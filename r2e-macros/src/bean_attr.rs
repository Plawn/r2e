use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ImplItem, ItemImpl, ReturnType, Type};

use crate::crate_path::r2e_core_path;

pub fn expand(input: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(input as ItemImpl);
    match generate(&item_impl) {
        Ok(bean_impl) => {
            // Emit the original impl with #[config] attrs stripped from constructor params
            let cleaned_impl = strip_config_attrs_from_constructor(&item_impl);
            let output = quote! {
                #cleaned_impl
                #bean_impl
            };
            output.into()
        }
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(item_impl: &ItemImpl) -> syn::Result<TokenStream2> {
    // Extract the Self type from the impl block.
    let self_ty = &item_impl.self_ty;

    // Find the constructor: a method that returns Self and has no self receiver.
    let (constructor, is_async) = find_constructor(item_impl)?;

    // Extract parameter types and generate dependency list + build args.
    let mut dep_type_ids = Vec::new();
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

                // Check for #[config("key")] attribute
                let config_attr = pat_type.attrs.iter().find(|a| a.path().is_ident("config"));

                if let Some(attr) = config_attr {
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
                } else {
                    dep_type_ids.push(quote! { (std::any::TypeId::of::<#ty>(), std::any::type_name::<#ty>()) });
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
    }

    let arg_forwards: Vec<_> = (0..build_args.len())
        .map(|i| {
            let arg_name = syn::Ident::new(&format!("__arg_{}", i), proc_macro2::Span::call_site());
            quote! { #arg_name }
        })
        .collect();

    let krate = r2e_core_path();

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

    if is_async {
        // Generate AsyncBean impl
        Ok(quote! {
            impl #krate::beans::AsyncBean for #self_ty {
                fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                    vec![#(#dep_type_ids),*]
                }

                #config_keys_fn

                async fn build(ctx: &#krate::beans::BeanContext) -> Self {
                    #config_prelude
                    #(#build_args)*
                    Self::#fn_name(#(#arg_forwards),*).await
                }
            }
        })
    } else {
        // Generate Bean impl (unchanged behavior)
        Ok(quote! {
            impl #krate::beans::Bean for #self_ty {
                fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                    vec![#(#dep_type_ids),*]
                }

                #config_keys_fn

                fn build(ctx: &#krate::beans::BeanContext) -> Self {
                    #config_prelude
                    #(#build_args)*
                    Self::#fn_name(#(#arg_forwards),*)
                }
            }
        })
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

/// Strip `#[config(...)]` attributes from the constructor parameters in the emitted impl block.
fn strip_config_attrs_from_constructor(item_impl: &ItemImpl) -> TokenStream2 {
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
                                .filter(|a| !a.path().is_ident("config"))
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
                items.push(quote! { #method });
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
