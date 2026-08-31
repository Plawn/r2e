//! `#[derive(ProvideBundle)]` — one `.provide_all(bundle)` call instead of one
//! `.provide(bundle.field)` line per field.
//!
//! The generated impl emits the hand-written chain verbatim, so the
//! compile-time provision list `P` grows with one entry per field, in field
//! order. A single `R2eConfig` field is applied as `override_config` instead of
//! being provided as a bean.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Type};

use crate::util::crate_path::r2e_core_path;

/// Textual match on the last path segment. `R2eConfig` is a concrete type in
/// `r2e-core` with no generic parameters, so the written spelling is one of
/// `R2eConfig`, `config::R2eConfig`, `r2e::R2eConfig`,
/// `r2e_core::config::R2eConfig`, … — all of which end in the same segment.
/// A type *alias* to `R2eConfig` (or an unrelated type named `R2eConfig`) is
/// therefore misread; spell the real path when that matters.
fn is_r2e_config(ty: &Type) -> bool {
    match ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "R2eConfig" && seg.arguments.is_none()),
        _ => false,
    }
}

pub fn expand(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let krate = r2e_core_path();

    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.generics,
            "#[derive(ProvideBundle)] does not support generic structs — the \
             provision list is a concrete type-level list, so every field type \
             must be known at the derive site",
        )
        .to_compile_error()
        .into();
    }

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            Fields::Unnamed(_) => {
                return syn::Error::new_spanned(
                    &s.fields,
                    "#[derive(ProvideBundle)] requires named fields — each field \
                     name is what the generated `.provide(bundle.field)` reads",
                )
                .to_compile_error()
                .into();
            }
            Fields::Unit => {
                return syn::Error::new_spanned(
                    name,
                    "#[derive(ProvideBundle)] on a unit struct provides nothing — \
                     drop the derive",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                name,
                "#[derive(ProvideBundle)] is only valid on structs",
            )
            .to_compile_error()
            .into();
        }
    };

    // Split the fields into the (at most one) config field and the beans.
    let mut config_field: Option<&syn::Field> = None;
    let mut bean_fields: Vec<&syn::Field> = Vec::new();
    for field in fields {
        if is_r2e_config(&field.ty) {
            if let Some(first) = config_field {
                let first_name = first.ident.as_ref().unwrap();
                return syn::Error::new_spanned(
                    field,
                    format!(
                        "a `ProvideBundle` may carry at most one `R2eConfig` field — \
                         `{first_name}` already claims it. The config field becomes \
                         `override_config(..)`, and a second one would silently \
                         discard the first"
                    ),
                )
                .to_compile_error()
                .into();
            }
            config_field = Some(field);
        } else {
            bean_fields.push(field);
        }
    }

    // `OutP`: `.provide()` pushes onto the FRONT of `P`, so the last field
    // provided ends up outermost.
    let mut out_p: TokenStream2 = quote! { __R2eP };
    for field in &bean_fields {
        let ty = &field.ty;
        out_p = quote! { #krate::type_list::TCons<#ty, #out_p> };
    }

    let config_stmt = match config_field {
        Some(field) => {
            let ident = field.ident.as_ref().unwrap();
            quote! { let __r2e_builder = __r2e_builder.override_config(self.#ident); }
        }
        None => quote! {},
    };

    let provide_stmts: Vec<TokenStream2> = bean_fields
        .iter()
        .map(|field| {
            let ident = field.ident.as_ref().unwrap();
            quote! { let __r2e_builder = __r2e_builder.provide(self.#ident); }
        })
        .collect();

    quote! {
        impl<__R2eP, __R2eR, __R2eMods>
            #krate::di::bundle::ProvideBundle<__R2eP, __R2eR, __R2eMods> for #name
        {
            type OutP = #out_p;

            fn provide_into(
                self,
                __r2e_builder: #krate::AppBuilder<
                    #krate::builder::NoState, __R2eP, __R2eR, __R2eMods,
                >,
            ) -> #krate::AppBuilder<
                #krate::builder::NoState, Self::OutP, __R2eR, __R2eMods,
            > {
                #config_stmt
                #(#provide_stmts)*
                __r2e_builder
            }
        }
    }
    .into()
}
