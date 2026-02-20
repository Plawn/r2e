use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

/// Build a `TCons<A, TCons<B, ... TNil>>` type from a list of types.
///
/// The types are folded right-to-left so the first element in `types`
/// becomes the outermost `TCons` head.
pub fn build_tcons_type(types: &[TokenStream2], krate: &TokenStream2) -> TokenStream2 {
    let mut result = quote! { #krate::type_list::TNil };
    for ty in types.iter().rev() {
        result = quote! { #krate::type_list::TCons<#ty, #result> };
    }
    result
}
