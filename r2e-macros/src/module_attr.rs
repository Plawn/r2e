//! `#[module(...)]` attribute macro — generates a `FeatureModule` impl from a
//! declarative listing of providers, controllers, exports, and imports.
//!
//! ```ignore
//! #[module(
//!     providers(UserRepo, UserService),
//!     controllers(UserController, AdminController),
//!     exports(UserService),
//!     imports(DbPool, module(BillingModule)),
//! )]
//! pub struct UserModule;
//! ```
//!
//! Every key is optional and defaults to empty. `providers`, `exports`, and
//! `imports` become `TCons` type-level lists; `controllers` becomes a tuple.
//!
//! An `imports(...)` entry is either a bean type or `module(OtherModule)`: the
//! latter appends the imported module's `Exports` to this module's import list
//! (via `TAppend`), so composing modules never has to restate the exported bean
//! types. `module(A, B)` and repeated `module(A), module(B)` are equivalent.
//! Importing a module only *requires* its exports — it does NOT register that
//! module; the app must still `.register_module::<OtherModule>()`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{parenthesized, token, Ident, ItemStruct, Token, Type};

use crate::crate_path::r2e_core_path;
use crate::type_list_gen::build_tcons_type;

#[derive(Default)]
struct ModuleArgs {
    providers: Vec<Type>,
    controllers: Vec<Type>,
    exports: Vec<Type>,
    /// Plain bean types listed in `imports(...)`.
    imports: Vec<Type>,
    /// Module types imported via `imports(module(...))` — their `Exports` are
    /// appended to `Imports`.
    import_modules: Vec<Type>,
    requires_plugins: Vec<Type>,
}

/// One entry in a declaration key's parenthesized list: either a plain bean
/// type or a `module(A, B, ...)` group. Only `imports(...)` accepts the latter.
enum Entry {
    Bean(Type),
    Modules { span: Span, types: Vec<Type> },
}

impl Parse for Entry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // `module` alone parses as a bare `Type` (an ident path), so peek for
        // the `module` ident *immediately followed by a paren group* to
        // disambiguate it from a type named `module` (or `module::Foo`).
        if input.peek(Ident) && input.peek2(token::Paren) {
            let fork = input.fork();
            let ident: Ident = fork.parse()?;
            if ident == "module" {
                let ident: Ident = input.parse()?;
                let content;
                parenthesized!(content in input);
                let types: Vec<Type> = Punctuated::<Type, Token![,]>::parse_terminated(&content)?
                    .into_iter()
                    .collect();
                return Ok(Entry::Modules {
                    span: ident.span(),
                    types,
                });
            }
        }
        Ok(Entry::Bean(input.parse()?))
    }
}

/// Reject `module(...)` in every key but `imports(...)`.
fn beans_only(entries: Vec<Entry>, key: &str) -> syn::Result<Vec<Type>> {
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Entry::Bean(ty) => out.push(ty),
            Entry::Modules { span, .. } => {
                return Err(syn::Error::new(
                    span,
                    format!(
                        "`module(...)` is only valid inside `imports(...)`, not `{key}(...)` \
                         — module imports go in `imports(...)`"
                    ),
                ));
            }
        }
    }
    Ok(out)
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
            let entries: Vec<Entry> = Punctuated::<Entry, Token![,]>::parse_terminated(&content)?
                .into_iter()
                .collect();

            match key_name.as_str() {
                "providers" => args.providers = beans_only(entries, "providers")?,
                "controllers" => args.controllers = beans_only(entries, "controllers")?,
                "exports" => args.exports = beans_only(entries, "exports")?,
                "imports" => {
                    for entry in entries {
                        match entry {
                            Entry::Bean(ty) => args.imports.push(ty),
                            Entry::Modules { types, .. } => args.import_modules.extend(types),
                        }
                    }
                }
                "requires_plugins" => {
                    args.requires_plugins = beans_only(entries, "requires_plugins")?
                }
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

    let to_tokens =
        |types: &[Type]| -> Vec<TokenStream2> { types.iter().map(|ty| quote! { #ty }).collect() };
    let providers = build_tcons_type(&to_tokens(&args.providers), &krate);
    let exports = build_tcons_type(&to_tokens(&args.exports), &krate);

    // `Imports` starts as the `TCons` list of plain bean imports, then chains a
    // `TAppend` of each imported module's `Exports`:
    //   <TCons<DbPool, TNil> as TAppend<<Billing as FeatureModule>::Exports>>::Output
    let mut imports = build_tcons_type(&to_tokens(&args.imports), &krate);
    for module in &args.import_modules {
        imports = quote! {
            <#imports as #krate::type_list::TAppend<
                <#module as #krate::module::FeatureModule>::Exports
            >>::Output
        };
    }

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
