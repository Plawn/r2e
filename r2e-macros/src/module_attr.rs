//! `#[module(...)]` attribute macro — generates a `FeatureModule` impl from a
//! declarative listing of providers, controllers, exports, and imports.
//!
//! ```ignore
//! #[module(
//!     providers(UserRepo, UserService),
//!     controllers(UserController, AdminController),
//!     exports(UserService),
//!     imports(DbPool),
//! )]
//! pub struct UserModule;
//! ```
//!
//! Every key is optional and defaults to empty. `providers`, `exports`, and
//! `imports` become `TCons` type-level lists; `controllers` becomes a tuple.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{parenthesized, Ident, ItemStruct, Token, Type};

use crate::crate_path::r2e_core_path;
use crate::type_list_gen::build_tcons_type;

#[derive(Default)]
struct ModuleArgs {
    providers: Vec<Type>,
    controllers: Vec<Type>,
    exports: Vec<Type>,
    imports: Vec<Type>,
    requires_plugins: Vec<Type>,
}

impl Parse for ModuleArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ModuleArgs::default();
        let mut seen: Vec<String> = Vec::new();

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let key_name = key.to_string();

            if seen.contains(&key_name) {
                return Err(syn::Error::new(
                    key.span(),
                    format!("duplicate `{key_name}(...)` in #[module]"),
                ));
            }

            let content;
            parenthesized!(content in input);
            let types: Vec<Type> = Punctuated::<Type, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect();

            match key_name.as_str() {
                "providers" => args.providers = types,
                "controllers" => args.controllers = types,
                "exports" => args.exports = types,
                "imports" => args.imports = types,
                "requires_plugins" => args.requires_plugins = types,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown key `{other}` in #[module] — expected `providers`, \
                             `controllers`, `exports`, `imports`, or `requires_plugins`"
                        ),
                    ));
                }
            }
            seen.push(key_name);

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(args)
    }
}

pub fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = syn::parse_macro_input!(args as ModuleArgs);
    let item = syn::parse_macro_input!(input as ItemStruct);

    if !item.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &item.generics,
            "#[module] does not support generic structs",
        )
        .to_compile_error()
        .into();
    }

    let name = &item.ident;
    let krate = r2e_core_path();

    let to_tokens = |types: &[Type]| -> Vec<TokenStream2> {
        types.iter().map(|ty| quote! { #ty }).collect()
    };
    let providers = build_tcons_type(&to_tokens(&args.providers), &krate);
    let exports = build_tcons_type(&to_tokens(&args.exports), &krate);
    let imports = build_tcons_type(&to_tokens(&args.imports), &krate);

    // 0 controllers → `()`, 1 → `(C,)`, n → `(C0, ..., Cn)` — the trailing
    // comma keeps the single-element case a tuple.
    let controller_types = &args.controllers;
    let controllers = quote! { ( #(#controller_types,)* ) };

    // `RequiredPlugins` is a tuple of plugin types (same shape as controllers).
    let required_plugin_types = &args.requires_plugins;
    let required_plugins = quote! { ( #(#required_plugin_types,)* ) };

    quote! {
        #item

        impl #krate::module::FeatureModule for #name {
            type Providers = #providers;
            type Controllers = #controllers;
            type Exports = #exports;
            type Imports = #imports;
            type RequiredPlugins = #required_plugins;
        }
    }
    .into()
}
