use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse_macro_input;

use crate::controller_codegen;
use crate::controller_parsing::{self, CONTROLLER_FIELD_ATTRS};

pub fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as syn::ItemStruct);
    let args = TokenStream2::from(args);

    // Emit the physical struct with the controller helper field attributes
    // stripped — they are consumed by this macro and are no longer registered
    // helper attributes once the derive is gone. The stripped struct is emitted
    // even on error so the user's type still exists, keeping diagnostics
    // targeted instead of triggering "cannot find type / unused import"
    // cascades from the failed expansion.
    let physical_struct = strip_field_attrs(item.clone());

    let generated = controller_parsing::parse_controller_args(args, item.ident.span())
        .and_then(|(state_type, prefix)| controller_parsing::parse(state_type, prefix, &item))
        .map(|def| controller_codegen::generate(&def, &physical_struct));

    match generated {
        Ok(tokens) => tokens.into(),
        Err(err) => {
            let err = err.to_compile_error();
            quote! {
                #physical_struct
                #err
            }
            .into()
        }
    }
}

/// Remove the controller field helper attributes from each field of the struct.
fn strip_field_attrs(mut item: syn::ItemStruct) -> syn::ItemStruct {
    if let syn::Fields::Named(named) = &mut item.fields {
        for field in named.named.iter_mut() {
            field.attrs.retain(|attr| {
                !CONTROLLER_FIELD_ATTRS
                    .iter()
                    .any(|n| attr.path().is_ident(n))
            });
        }
    }
    item
}
