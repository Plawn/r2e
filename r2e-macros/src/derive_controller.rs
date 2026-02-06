use proc_macro::TokenStream;
use syn::parse_macro_input;

use crate::derive_codegen;
use crate::derive_parsing;

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    match derive_parsing::parse(input) {
        Ok(def) => derive_codegen::generate(&def).into(),
        Err(err) => err.to_compile_error().into(),
    }
}
