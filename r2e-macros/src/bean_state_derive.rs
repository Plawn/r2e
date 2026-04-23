use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use std::collections::HashSet;
use syn::{parse_macro_input, punctuated::Punctuated, token::Comma, Data, DeriveInput, Field, Fields, Ident, Type};

use crate::crate_path::r2e_core_path;

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate(&input) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Generate `FromRef` impls for each unique field type, skipping fields
/// annotated with `#[<skip_attr_name>(skip)]` or `#[<skip_attr_name>(skip_from_ref)]`.
///
/// Shared between `BeanState` and `TestState` derives.
pub fn generate_from_ref_impls(
    name: &Ident,
    fields: &Punctuated<Field, Comma>,
    skip_attr_name: &str,
) -> Vec<TokenStream2> {
    let krate = r2e_core_path();
    let mut seen_types = HashSet::new();
    let mut from_ref_impls = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        // Check for #[<skip_attr_name>(skip)] or #[<skip_attr_name>(skip_from_ref)]
        let skip = field.attrs.iter().any(|attr| {
            if !attr.path().is_ident(skip_attr_name) {
                return false;
            }
            attr.parse_args::<syn::Ident>()
                .map(|ident| ident == "skip" || ident == "skip_from_ref")
                .unwrap_or(false)
        });

        if skip {
            continue;
        }

        // Use the stringified type as the dedup key.
        let type_key = type_to_string(field_type);
        if !seen_types.insert(type_key) {
            continue;
        }

        from_ref_impls.push(quote! {
            impl #krate::http::extract::FromRef<#name> for #field_type {
                fn from_ref(state: &#name) -> Self {
                    state.#field_name.clone()
                }
            }
        });
    }

    from_ref_impls
}

fn generate(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(BeanState)] only works on structs with named fields:\n\
                     \n  #[derive(BeanState, Clone)]\n  struct AppState {\n      service: MyService,\n      pool: SqlitePool,\n  }",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(BeanState)] only works on structs — enums and unions are not supported",
            ))
        }
    };

    let krate = r2e_core_path();

    let field_inits: Vec<TokenStream2> = fields
        .iter()
        .map(|f| {
            let field_name = f.ident.as_ref().unwrap();
            let field_type = &f.ty;
            quote! { #field_name: ctx.get::<#field_type>() }
        })
        .collect();

    let from_ref_impls = generate_from_ref_impls(name, fields, "bean_state");

    // `Option<T>` fields produce a bound on `Option<T>` itself — a producer
    // must register `Option<T>` for the state to build.
    let mut buildable_seen = HashSet::new();
    let mut idx_params = Vec::new();
    let mut buildable_bounds = Vec::new();
    let mut idx_counter = 0usize;
    for field in fields {
        let field_type = &field.ty;
        if buildable_seen.insert(type_to_string(field_type)) {
            let idx_ident = quote::format_ident!("__I{}", idx_counter);
            idx_counter += 1;
            idx_params.push(quote! { #idx_ident });
            buildable_bounds.push(quote! {
                __P: #krate::type_list::Contains<#field_type, #idx_ident>
            });
        }
    }

    // Bundle index witnesses into a tuple for the Indices parameter.
    let indices_tuple = if idx_params.is_empty() {
        quote! { () }
    } else {
        quote! { (#(#idx_params,)*) }
    };

    Ok(quote! {
        impl #krate::beans::BeanState for #name {
            fn from_context(ctx: &#krate::beans::BeanContext) -> Self {
                Self {
                    #(#field_inits,)*
                }
            }
        }

        impl<__P, #(#idx_params,)*> #krate::type_list::BuildableFrom<__P, #indices_tuple> for #name
        where
            #(#buildable_bounds,)*
        {}

        #(#from_ref_impls)*
    })
}

/// Produce a stable string representation of a type for dedup purposes.
fn type_to_string(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}
