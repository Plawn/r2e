use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, FnArg, ImplItem, ItemImpl, ReturnType, Type};

use crate::crate_path::r2e_core_path;

pub fn expand(input: TokenStream) -> TokenStream {
    let item_impl = parse_macro_input!(input as ItemImpl);
    match generate(&item_impl) {
        Ok(bean_impl) => {
            let output = quote! {
                #item_impl
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
    let constructor = find_constructor(item_impl)?;

    // Extract parameter types and generate dependency list + build args.
    let mut dep_type_ids = Vec::new();
    let mut build_args = Vec::new();

    let fn_name = &constructor.sig.ident;

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
                dep_type_ids.push(quote! { (std::any::TypeId::of::<#ty>(), std::any::type_name::<#ty>()) });
                build_args.push(quote! { let #arg_name: #ty = ctx.get::<#ty>(); });
            }
        }
    }

    let arg_forwards: Vec<_> = (0..build_args.len())
        .map(|i| {
            let arg_name = syn::Ident::new(&format!("__arg_{}", i), proc_macro2::Span::call_site());
            quote! { #arg_name }
        })
        .collect();

    let krate = r2e_core_path();
    Ok(quote! {
        impl #krate::beans::Bean for #self_ty {
            fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                vec![#(#dep_type_ids),*]
            }

            fn build(ctx: &#krate::beans::BeanContext) -> Self {
                #(#build_args)*
                Self::#fn_name(#(#arg_forwards),*)
            }
        }
    })
}

/// Find the constructor method in the impl block.
///
/// The constructor is the first associated function (no `self` receiver)
/// whose return type is `Self` or matches the impl type name.
fn find_constructor(item_impl: &ItemImpl) -> syn::Result<&syn::ImplItemFn> {
    for item in &item_impl.items {
        if let ImplItem::Fn(method) = item {
            // Skip methods with a self receiver.
            if method.sig.inputs.iter().any(|arg| matches!(arg, FnArg::Receiver(_))) {
                continue;
            }

            // Check return type is Self or the type name.
            if returns_self(&method.sig.output, &item_impl.self_ty) {
                return Ok(method);
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
