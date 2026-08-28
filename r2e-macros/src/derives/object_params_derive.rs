use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

use crate::util::crate_path::r2e_mcp_path;

pub fn expand(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_inner(input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_inner(input: DeriveInput) -> syn::Result<TokenStream> {
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "ObjectParams can only be derived for structs with named fields",
        ));
    };
    if !matches!(data.fields, Fields::Named(_)) {
        return Err(syn::Error::new_spanned(
            &input.ident,
            "ObjectParams can only be derived for structs with named fields",
        ));
    }

    let mcp = r2e_mcp_path();
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #mcp::__macro_support::ObjectParamsSeal
            for #name #ty_generics #where_clause
        {}

        impl #impl_generics #mcp::ObjectParams for #name #ty_generics #where_clause {}
    })
}
