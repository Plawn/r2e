use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use std::collections::HashSet;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Type};

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
                    "#[derive(BeanState)] only works on structs with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(BeanState)] only works on structs",
            ))
        }
    };

    // Generate BeanState::from_context() — all fields resolved from context.
    let field_inits: Vec<TokenStream2> = fields
        .iter()
        .map(|f| {
            let field_name = f.ident.as_ref().unwrap();
            let field_type = &f.ty;
            quote! { #field_name: ctx.get::<#field_type>() }
        })
        .collect();

    // Generate FromRef impls for each unique field type, unless the field
    // is annotated with #[bean_state(skip_from_ref)].
    let mut seen_types = HashSet::new();
    let mut from_ref_impls = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        // Check for #[bean_state(skip_from_ref)]
        let skip = field.attrs.iter().any(|attr| {
            if !attr.path().is_ident("bean_state") {
                return false;
            }
            attr.parse_args::<syn::Ident>()
                .map(|ident| ident == "skip_from_ref")
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
            impl quarlus_core::http::extract::FromRef<#name> for #field_type {
                fn from_ref(state: &#name) -> Self {
                    state.#field_name.clone()
                }
            }
        });
    }

    Ok(quote! {
        impl quarlus_core::beans::BeanState for #name {
            fn from_context(ctx: &quarlus_core::beans::BeanContext) -> Self {
                Self {
                    #(#field_inits,)*
                }
            }
        }

        #(#from_ref_impls)*
    })
}

/// Produce a stable string representation of a type for dedup purposes.
fn type_to_string(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}
