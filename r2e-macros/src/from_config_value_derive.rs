use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

use crate::crate_path::r2e_core_path;

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate(&input) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let krate = r2e_core_path();

    Ok(quote! {
        impl #impl_generics #krate::config::FromConfigValue for #name #ty_generics #where_clause {
            fn from_config_value(
                value: &#krate::config::ConfigValue,
                key: &str,
            ) -> Result<Self, #krate::config::ConfigError> {
                #krate::config::deserialize_value::<Self>(value, key)
            }
        }
    })
}
