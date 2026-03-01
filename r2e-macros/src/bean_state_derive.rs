use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use std::collections::HashSet;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Type};

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

        let krate = r2e_core_path();
        from_ref_impls.push(quote! {
            impl #krate::http::extract::FromRef<#name> for #field_type {
                fn from_ref(state: &#name) -> Self {
                    state.#field_name.clone()
                }
            }
        });

        // If this is R2eConfig<T> (with generic args), also generate
        // FromRef for the raw R2eConfig (= R2eConfig<()>) so that
        // beans and controllers can extract the untyped config via FromRef.
        if is_typed_r2e_config(field_type) {
            let raw_type_key = format!("{}::config::R2eConfig", quote!(#krate));
            if seen_types.insert(raw_type_key) {
                from_ref_impls.push(quote! {
                    impl #krate::http::extract::FromRef<#name> for #krate::config::R2eConfig {
                        fn from_ref(state: &#name) -> Self {
                            state.#field_name.raw()
                        }
                    }
                });
            }
        }
    }

    // Generate BuildableFrom<P, Indices> impl with index witness type params.
    // Each unique field type gets its own __I{n} parameter bundled into a tuple
    // so the compiler can independently resolve Contains<FieldType, __I{n}>.
    let krate = r2e_core_path();

    let mut buildable_seen = HashSet::new();
    let mut idx_params = Vec::new();
    let mut buildable_bounds = Vec::new();
    let mut idx_counter = 0usize;
    for field in fields {
        let field_type = &field.ty;
        let type_key = type_to_string(field_type);
        if buildable_seen.insert(type_key) {
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

/// Check if a type is `R2eConfig<T>` with explicit generic arguments.
///
/// Returns `true` for `R2eConfig<AppConfig>` but `false` for bare `R2eConfig`.
fn is_typed_r2e_config(ty: &Type) -> bool {
    if let Type::Path(syn::TypePath { path, .. }) = ty {
        if let Some(seg) = path.segments.last() {
            if seg.ident == "R2eConfig" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    return !args.args.is_empty();
                }
            }
        }
    }
    false
}
