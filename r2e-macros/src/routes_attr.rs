use proc_macro::TokenStream;
use syn::parse_macro_input;

use crate::codegen;
use crate::routes_parsing;

pub fn expand(input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as syn::ItemImpl);
    match routes_parsing::parse(item) {
        Ok(def) => codegen::generate(&def).into(),
        Err(err) => err.to_compile_error().into(),
    }
}
