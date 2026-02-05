use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::crate_path::quarlus_core_path;

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate(&input) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(Bean)] only works on structs with named fields:\n\
                     \n  #[derive(Bean, Clone)]\n  struct MyService {\n      #[inject] dep: OtherService,\n  }",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(Bean)] only works on structs — enums and unions are not supported",
            ))
        }
    };

    let mut dep_type_ids = Vec::new();
    let mut field_inits = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        let is_inject = field.attrs.iter().any(|a| a.path().is_ident("inject"));

        if is_inject {
            dep_type_ids.push(quote! { (std::any::TypeId::of::<#field_type>(), std::any::type_name::<#field_type>()) });
            field_inits.push(quote! { #field_name: ctx.get::<#field_type>() });
        } else {
            // Fields without #[inject] use Default::default()
            field_inits.push(quote! { #field_name: Default::default() });
        }
    }

    let krate = quarlus_core_path();
    Ok(quote! {
        impl #krate::beans::Bean for #name {
            fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                vec![#(#dep_type_ids),*]
            }

            fn build(ctx: &#krate::beans::BeanContext) -> Self {
                Self {
                    #(#field_inits,)*
                }
            }
        }
    })
}
