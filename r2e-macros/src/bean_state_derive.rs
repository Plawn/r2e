//! Shared helpers for state-struct derives (`TestState`): per-field `FromRef`
//! impls and the HList-model bridge impls (`HasBean<T, ByField>` /
//! `Contains` / `BeanLookup`).

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use std::collections::HashSet;
use syn::{punctuated::Punctuated, token::Comma, Field, Ident, Type};

use crate::crate_path::r2e_core_path;

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

/// Transitional bridge to the HList state model (Phase 4): typed state
/// structs satisfy the same by-type access bounds as HList states —
/// `HasBean<T, ByField>` + `Contains<T, ByField>` per unique field type
/// (used by the state-generic controller codegen and the `Deps` presence
/// check at `register_controller`), plus `BeanLookup` for guards and
/// managed resources that look beans up dynamically.
///
/// Fields skipped for `FromRef` (`#[<attr>(skip)]` / `skip_from_ref`) are
/// skipped here too. Used by the `TestState` derive.
pub fn generate_state_bridge_impls(
    name: &Ident,
    fields: &Punctuated<Field, Comma>,
    skip_attr_name: &str,
) -> TokenStream2 {
    let krate = r2e_core_path();
    let mut bridge_seen = HashSet::new();
    let mut has_bean_impls: Vec<TokenStream2> = Vec::new();
    let mut lookup_arms: Vec<TokenStream2> = Vec::new();
    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;
        let skip = field.attrs.iter().any(|attr| {
            if !attr.path().is_ident(skip_attr_name) {
                return false;
            }
            attr.parse_args::<syn::Ident>()
                .map(|ident| ident == "skip" || ident == "skip_from_ref")
                .unwrap_or(false)
        });
        if skip || !bridge_seen.insert(type_to_string(field_type)) {
            continue;
        }
        has_bean_impls.push(quote! {
            impl #krate::type_list::HasBean<#field_type, #krate::type_list::ByField> for #name {
                #[inline]
                fn get_bean(&self) -> #field_type {
                    self.#field_name.clone()
                }
            }

            impl #krate::type_list::Contains<#field_type, #krate::type_list::ByField> for #name {}
        });
        lookup_arms.push(quote! {
            if ::std::any::TypeId::of::<#field_type>() == tid {
                return Some(&self.#field_name);
            }
        });
    }
    quote! {
        #(#has_bean_impls)*

        impl #krate::type_list::BeanLookup for #name {
            fn lookup_bean(
                &self,
                tid: ::std::any::TypeId,
            ) -> Option<&(dyn ::std::any::Any + Send + Sync)> {
                #(#lookup_arms)*
                None
            }
        }
    }
}

/// Produce a stable string representation of a type for dedup purposes.
fn type_to_string(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}
