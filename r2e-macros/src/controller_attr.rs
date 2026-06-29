use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse_macro_input;

use crate::controller_codegen;
use crate::controller_parsing::{self, CONTROLLER_FIELD_ATTRS};

pub fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as syn::ItemStruct);
    let args = TokenStream2::from(args);

    let parsed = controller_parsing::parse_controller_args(args, item.ident.span())
        .and_then(|(state_type, prefix)| controller_parsing::parse(state_type, prefix, &item));

    match parsed {
        Ok(def) => {
            // The physical core keeps only app/config-scoped fields — every
            // request-scoped field (identity + `#[inject(request)]`) is removed
            // and lives on the generated request façade instead.
            let removed = def.request_scoped_field_names();
            let physical_struct = strip_field_attrs(item, &removed);
            controller_codegen::generate(&def, &physical_struct).into()
        }
        Err(err) => {
            // On error, still emit a struct with helper attributes stripped (but
            // all fields kept) so the user's type exists and diagnostics stay
            // targeted instead of triggering "cannot find type" cascades.
            let physical_struct = strip_field_attrs(item, &[]);
            let err = err.to_compile_error();
            quote! {
                #physical_struct
                #err
            }
            .into()
        }
    }
}

/// Remove the controller field helper attributes from each field, and drop the
/// fields named in `removed` entirely (request-scoped fields moved to the façade).
fn strip_field_attrs(mut item: syn::ItemStruct, removed: &[syn::Ident]) -> syn::ItemStruct {
    if let syn::Fields::Named(named) = &mut item.fields {
        named.named = std::mem::take(&mut named.named)
            .into_iter()
            .filter(|field| {
                field
                    .ident
                    .as_ref()
                    .map(|id| !removed.iter().any(|r| r == id))
                    .unwrap_or(true)
            })
            .map(|mut field| {
                field.attrs.retain(|attr| {
                    !CONTROLLER_FIELD_ATTRS
                        .iter()
                        .any(|n| attr.path().is_ident(n))
                });
                field
            })
            .collect();
    }
    item
}
