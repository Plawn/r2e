use proc_macro::TokenStream;
use syn::parse_macro_input;

use crate::codegen;
use crate::parsing::ControllerDef;

pub fn expand(input: TokenStream) -> TokenStream {
    let def = parse_macro_input!(input as ControllerDef);
    codegen::generate(&def).into()
}
