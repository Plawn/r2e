//! Crate path resolution for generated code.
//!
//! Detects whether the user depends on `r2e` (facade) or `r2e-core` directly,
//! and returns the appropriate path prefix for generated code.

use proc_macro2::TokenStream;
use proc_macro_crate::{crate_name, FoundCrate};
use quote::quote;

/// Returns the token stream for accessing `r2e_core` types.
///
/// If the user depends on `r2e`, returns `::r2e`.
/// Otherwise returns `::r2e_core`.
pub fn r2e_core_path() -> TokenStream {
    // First check if the facade crate is available
    if let Ok(found) = crate_name("r2e") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else if let Ok(found) = crate_name("r2e-core") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        // Fallback - assume r2e_core is available (for error messages)
        quote!(::r2e_core)
    }
}

/// Returns the token stream for accessing `r2e_security` types.
///
/// If the user depends on `r2e`, returns `::r2e::r2e_security`.
/// Otherwise returns `::r2e_security`.
pub fn r2e_security_path() -> TokenStream {
    // First check if the facade crate is available
    if let Ok(found) = crate_name("r2e") {
        match found {
            FoundCrate::Itself => quote!(crate::r2e_security),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident::r2e_security)
            }
        }
    } else if let Ok(found) = crate_name("r2e-security") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        // Fallback
        quote!(::r2e_security)
    }
}

/// Returns the token stream for accessing `r2e_scheduler` types.
///
/// If the user depends on `r2e`, returns `::r2e::r2e_scheduler`.
/// Otherwise returns `::r2e_scheduler`.
pub fn r2e_scheduler_path() -> TokenStream {
    // First check if the facade crate is available
    if let Ok(found) = crate_name("r2e") {
        match found {
            FoundCrate::Itself => quote!(crate::r2e_scheduler),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident::r2e_scheduler)
            }
        }
    } else if let Ok(found) = crate_name("r2e-scheduler") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        // Fallback
        quote!(::r2e_scheduler)
    }
}

/// Returns the token stream for accessing `r2e_grpc` types.
///
/// If the user depends on `r2e`, returns `::r2e::r2e_grpc`.
/// Otherwise returns `::r2e_grpc`.
pub fn r2e_grpc_path() -> TokenStream {
    // First check if the facade crate is available
    if let Ok(found) = crate_name("r2e") {
        match found {
            FoundCrate::Itself => quote!(crate::r2e_grpc),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident::r2e_grpc)
            }
        }
    } else if let Ok(found) = crate_name("r2e-grpc") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        // Fallback
        quote!(::r2e_grpc)
    }
}
