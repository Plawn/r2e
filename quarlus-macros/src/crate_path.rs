//! Crate path resolution for generated code.
//!
//! Detects whether the user depends on `quarlus` (facade) or `quarlus-core` directly,
//! and returns the appropriate path prefix for generated code.

use proc_macro2::TokenStream;
use proc_macro_crate::{crate_name, FoundCrate};
use quote::quote;

/// Returns the token stream for accessing `quarlus_core` types.
///
/// If the user depends on `quarlus`, returns `::quarlus`.
/// Otherwise returns `::quarlus_core`.
pub fn quarlus_core_path() -> TokenStream {
    // First check if the facade crate is available
    if let Ok(found) = crate_name("quarlus") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else if let Ok(found) = crate_name("quarlus-core") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        // Fallback - assume quarlus_core is available (for error messages)
        quote!(::quarlus_core)
    }
}

/// Returns the token stream for accessing `quarlus_rate_limit` types.
///
/// If the user depends on `quarlus`, returns `::quarlus::quarlus_rate_limit`.
/// Otherwise returns `::quarlus_rate_limit`.
pub fn quarlus_rate_limit_path() -> TokenStream {
    // First check if the facade crate is available
    if let Ok(found) = crate_name("quarlus") {
        match found {
            FoundCrate::Itself => quote!(crate::quarlus_rate_limit),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident::quarlus_rate_limit)
            }
        }
    } else if let Ok(found) = crate_name("quarlus-rate-limit") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        // Fallback
        quote!(::quarlus_rate_limit)
    }
}
