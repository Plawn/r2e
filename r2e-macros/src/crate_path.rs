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

/// Returns the token stream for accessing `r2e_events` types.
///
/// If the user depends on `r2e`, returns `::r2e::r2e_events`.
/// Otherwise returns `::r2e_events`.
pub fn r2e_events_path() -> TokenStream {
    if let Ok(found) = crate_name("r2e") {
        match found {
            FoundCrate::Itself => quote!(crate::r2e_events),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident::r2e_events)
            }
        }
    } else if let Ok(found) = crate_name("r2e-events") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        quote!(::r2e_events)
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

/// Returns the token stream for accessing `r2e_devtools` types.
///
/// If the user depends on `r2e`, returns `::r2e::devtools`.
/// Otherwise returns `::r2e_devtools`.
pub fn r2e_devtools_path() -> TokenStream {
    if let Ok(found) = crate_name("r2e") {
        match found {
            FoundCrate::Itself => quote!(crate::devtools),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident::devtools)
            }
        }
    } else if let Ok(found) = crate_name("r2e-devtools") {
        match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        }
    } else {
        // Fallback
        quote!(::r2e_devtools)
    }
}

/// Returns the token stream for accessing `schemars` through `r2e-openapi`.
///
/// Resolution order:
/// 1. Direct `schemars` dependency → `::schemars`
/// 2. Direct `r2e-openapi` dependency → `::r2e_openapi::schemars`
/// 3. `r2e` facade → `::r2e::r2e_openapi::schemars`
///
/// Returns `None` if no path is found.
pub fn r2e_schemars_path() -> Option<TokenStream> {
    // Direct schemars dep
    if let Ok(found) = crate_name("schemars") {
        let p = match found {
            FoundCrate::Itself => quote!(crate),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident)
            }
        };
        return Some(p);
    }

    // Through r2e-openapi
    if let Ok(found) = crate_name("r2e-openapi") {
        let p = match found {
            FoundCrate::Itself => quote!(crate::schemars),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident::schemars)
            }
        };
        return Some(p);
    }

    // Through r2e facade (assumes openapi feature is enabled)
    if let Ok(found) = crate_name("r2e") {
        let p = match found {
            FoundCrate::Itself => quote!(crate::r2e_openapi::schemars),
            FoundCrate::Name(name) => {
                let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
                quote!(::#ident::r2e_openapi::schemars)
            }
        };
        return Some(p);
    }

    None
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
