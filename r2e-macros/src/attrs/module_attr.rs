//! `#[module(...)]` attribute macro — generates a `FeatureModule` impl from a
//! declarative listing of providers, controllers, exports, and imports.
//!
//! ```ignore
//! #[module(
//!     providers(UserRepo, UserService),
//!     controllers(UserController, AdminController),
//!     grpc_services(UserGrpcService),
//!     exports(UserService),
//!     imports(DbPool, module(BillingModule)),
//!     plugins(Scheduler = Scheduler),
//!     requires_plugins(Executor),
//! )]
//! pub struct UserModule;
//! ```
//!
//! Every key is optional and defaults to empty. `providers`, `exports`, and
//! `imports` become `TCons` type-level lists; `controllers`, `grpc_services`,
//! `requires_plugins`, and `plugins` become tuples.
//!
//! `grpc_services(...)` — the gRPC services the module owns, the transport peer
//! of `controllers(...)`. They are dependency-checked against the module scope
//! at `register_module` (so they may inject the module's *private* providers)
//! and registered by `build_state()` from the retained bean context. Declaring
//! any also adds `GrpcServer` to `RequiredPlugins`, so forgetting
//! `.plugin(GrpcServer::...)` is a compile error naming the plugin.
//!
//! `plugins(...)` — the plugins the module **brings** — takes `Type = expr`
//! entries: the type grows the module's provision list at compile time, the
//! expression is the instance installed at `register_module`. A module that
//! merely *needs* a plugin someone else installs uses
//! `requires_plugins(Type)` instead.
//!
//! An `imports(...)` entry is either a bean type or `module(OtherModule)`: the
//! latter appends the imported module's `Exports` to this module's import list
//! (via `TAppend`), so composing modules never has to restate the exported bean
//! types. `module(A, B)` and repeated `module(A), module(B)` are equivalent.
//! Importing a module only *requires* its exports — it does NOT register that
//! module; the app must still `.register_module::<OtherModule>()`.
//!
//! `modules(...)` — the **aggregate** form. A type declared as
//!
//! ```ignore
//! #[module(modules(UserModule, OrderModule, BillingModule))]
//! pub struct AppModules;
//! ```
//!
//! is not a module: it owns no providers, controllers or scope, so
//! `modules(...)` is exclusive with every other key. It generates a
//! `ModuleAggregate` impl and is registered with
//! `.register_modules::<AppModules>()`, which folds `register_module` over the
//! members in the listed order — exactly as if each had been registered by
//! hand. This is what lets an app and its tests share one blueprint line.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{parenthesized, token, Expr, Ident, ItemStruct, Token, Type};

use crate::model::type_list_gen::build_tcons_type;
use crate::util::crate_path::{r2e_core_path, r2e_grpc_path};

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
    /// `plugins(Type = expr, ...)` — the plugins this module brings.
    plugins: Vec<PluginEntry>,
    /// `grpc_services(...)` — the gRPC services this module owns.
    grpc_services: Vec<Type>,
    /// `prefix = "/api/v1"` — the HTTP mount point for this module's
    /// controllers.
    prefix: Option<String>,
    /// `modules(...)` — an **aggregate**: this type composes other modules and
    /// declares nothing of its own. Exclusive with every other key.
    modules: Vec<Type>,
    /// Span of the `modules` key, for the exclusivity diagnostic.
    modules_span: Option<Span>,
}

/// One `plugins(...)` entry: `Type = expr`.
struct PluginEntry {
    ty: Type,
    value: Expr,
}

impl Parse for PluginEntry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty: Type = input.parse()?;
        if !input.peek(Token![=]) {
            return Err(syn::Error::new_spanned(
                &ty,
                "`plugins(...)` entries must be written `Type = expr` — e.g. \
                 `plugins(Scheduler = Scheduler::default())`. The plugin type is needed at \
                 compile time (it grows the provision list); the expression is the instance \
                 to install. If the module only *needs* a plugin installed elsewhere, use \
                 `requires_plugins(Scheduler)` instead",
            ));
        }
        input.parse::<Token![=]>()?;
        let value: Expr = input.parse()?;
        Ok(Self { ty, value })
    }
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

/// Reject a type listed twice in the same key.
///
/// Textual comparison of the written paths: it catches the copy/paste
/// (`grpc_services(S, S)`) that would otherwise register one endpoint twice,
/// and cannot see through aliases or differently-spelled paths for the same
/// type — those are caught at boot, where the check is on the resolved
/// endpoint name.
fn reject_repeats(types: &[Type], key: &str) -> syn::Result<()> {
    let mut seen: Vec<(String, &Type)> = Vec::new();
    for ty in types {
        let rendered = quote!(#ty).to_string();
        if seen.iter().any(|(s, _)| *s == rendered) {
            return Err(syn::Error::new_spanned(
                ty,
                format!(
                    "`{}` is listed twice in `{key}(...)` — each entry is registered once, \
                     so a repeat is always a mistake",
                    rendered.replace(' ', "")
                ),
            ));
        }
        seen.push((rendered, ty));
    }
    Ok(())
}

/// Check a `prefix = "…"` value and return it.
///
/// Kept strict on purpose: the prefix ends up in `Router::nest` and in every
/// published OpenAPI path, where a stray trailing slash or a path parameter
/// would produce routes nobody declared.
fn validate_prefix(lit: &syn::LitStr) -> syn::Result<String> {
    let value = lit.value();
    let err = |msg: &str| Err(syn::Error::new_spanned(lit, msg.to_string()));

    if value.is_empty() {
        return err("`prefix` must not be empty — drop the key to mount the module at the app root");
    }
    if !value.starts_with('/') {
        return err("`prefix` must start with `/` — e.g. `prefix = \"/api/v1\"`");
    }
    if value == "/" {
        return err(
            "`prefix = \"/\"` mounts at the app root — drop the key instead of writing it out",
        );
    }
    if value.ends_with('/') {
        return err("`prefix` must not end with `/` — write `prefix = \"/api/v1\"`");
    }
    if value.contains('{') || value.contains('}') || value.contains('*') {
        return err(
            "`prefix` must be a literal path — a path parameter or wildcard in a module prefix \
             would have to be extracted by every controller in the module. Put it on the \
             controller's own `#[controller(path = ...)]` instead",
        );
    }
    Ok(value)
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

            // `prefix` is the one key with a scalar value (`prefix = "/api"`);
            // every other key takes a parenthesized list.
            if key_name == "prefix" {
                if !input.peek(Token![=]) {
                    return Err(syn::Error::new(
                        key.span(),
                        "`prefix` takes a string value — write `#[module(prefix = \"/api/v1\")]`",
                    ));
                }
                input.parse::<Token![=]>()?;
                let lit: syn::LitStr = input.parse()?;
                args.prefix = Some(validate_prefix(&lit)?);
                seen.push(key_name);
                if !input.is_empty() {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }

            let content;
            parenthesized!(content in input);

            // `plugins(...)` takes `Type = expr` entries, every other key takes
            // a plain type list, so the content is parsed per key.
            if key_name == "plugins" {
                args.plugins = Punctuated::<PluginEntry, Token![,]>::parse_terminated(&content)?
                    .into_iter()
                    .collect();
                seen.push(key_name);
                if !input.is_empty() {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }

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
                "grpc_services" => {
                    let services = beans_only(entries, "grpc_services")?;
                    reject_repeats(&services, "grpc_services")?;
                    args.grpc_services = services;
                }
                "modules" => {
                    let modules = beans_only(entries, "modules")?;
                    reject_repeats(&modules, "modules")?;
                    if modules.is_empty() {
                        return Err(syn::Error::new(
                            key.span(),
                            "`modules(...)` must list at least one module — an empty aggregate \
                             registers nothing",
                        ));
                    }
                    args.modules = modules;
                    args.modules_span = Some(key.span());
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown key `{other}` in #[module] — expected `providers`, \
                             `controllers`, `grpc_services`, `exports`, `imports`, `plugins`, \
                             `requires_plugins`, `prefix`, or `modules`"
                        ),
                    ));
                }
            }
            seen.push(key_name);

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        // An aggregate composes only other modules: it owns no providers,
        // controllers, exports, imports or plugins of its own. Mixing the two
        // forms would silently drop one of them, so it is a hard error.
        if let Some(span) = args.modules_span {
            let other: Vec<&String> = seen.iter().filter(|k| *k != "modules").collect();
            if !other.is_empty() {
                let listed = other
                    .iter()
                    .map(|k| {
                        if *k == "prefix" {
                            "`prefix = \"...\"`".to_string()
                        } else {
                            format!("`{k}(...)`")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                // An aggregate mounts nothing itself: each member carries its own
                // prefix, so a prefix here would have nowhere to apply.
                let hint = if other.iter().any(|k| *k == "prefix") {
                    "an aggregate only composes other modules and mounts nothing of its own. \
                     Declare the prefix on each member module instead"
                } else {
                    "an aggregate only composes other modules. Move the \
                     providers/controllers/exports/imports/plugins into a real module and list \
                     that module here"
                };
                return Err(syn::Error::new(
                    span,
                    format!(
                        "`modules(...)` is exclusive with every other #[module] key (found \
                         {listed}) — {hint}"
                    ),
                ));
            }
        }

        Ok(args)
    }
}

fn to_tokens_of(types: &[Type]) -> Vec<TokenStream2> {
    types.iter().map(|ty| quote! { #ty }).collect()
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

    // ── Aggregate form: `#[module(modules(A, B, C))]` ──────────────────────
    //
    // Emits a `ModuleAggregate` impl instead of a `FeatureModule` one: the
    // aggregate is not itself a module (it has no providers, controllers or
    // scope), it is a named list registered by
    // `.register_modules::<Aggregate>()`, which folds `register_module` over
    // the members in order.
    if !args.modules.is_empty() {
        let members = build_tcons_type(&to_tokens_of(&args.modules), &krate);
        return quote! {
            #item

            impl #krate::di::module::ModuleAggregate for #name {
                type Modules = #members;
            }
        }
        .into();
    }

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
                <#module as #krate::di::module::FeatureModule>::Exports
            >>::Output
        };
    }

    // 0 controllers → `()`, 1 → `(C,)`, n → `(C0, ..., Cn)` — the trailing
    // comma keeps the single-element case a tuple.
    let controller_types = &args.controllers;
    let controllers = quote! { ( #(#controller_types,)* ) };

    // `Endpoints` — the module's non-HTTP transport endpoints. Empty (the
    // common case) keeps the generated impl free of any transport-crate path,
    // so a module in an app without `r2e-grpc` still compiles.
    //
    // A module owning gRPC services needs the `GrpcServer` plugin installed
    // before it (that is where the service registry lives), so `GrpcServer` is
    // appended to `RequiredPlugins`: a missing plugin is then the standard
    // module diagnostic naming it, instead of a boot-time failure. A module
    // that *brings* `GrpcServer` via `plugins(..)` still satisfies the check —
    // `RequiredPlugins` is verified against the post-fold provision list.
    let grpc_service_types = &args.grpc_services;
    let (endpoints, extra_required_plugins) = if grpc_service_types.is_empty() {
        (quote! { () }, quote! {})
    } else {
        let grpc = r2e_grpc_path();
        (
            quote! { #grpc::ModuleGrpcServices<( #(#grpc_service_types,)* )> },
            quote! { #grpc::GrpcServer, },
        )
    };

    // `RequiredPlugins` is a tuple of plugin types (same shape as controllers).
    let required_plugin_types = &args.requires_plugins;
    let required_plugins = quote! { ( #(#required_plugin_types,)* #extra_required_plugins ) };

    // `Plugins` is a tuple of the brought plugins' types; `plugins()` builds
    // the matching tuple of instances, in the same order.
    let plugin_types: Vec<&Type> = args.plugins.iter().map(|p| &p.ty).collect();
    let plugin_values: Vec<&Expr> = args.plugins.iter().map(|p| &p.value).collect();
    let plugins_ty = quote! { ( #(#plugin_types,)* ) };
    // Empty → an empty body (returns `()`), so the common no-plugin case does
    // not emit a bare unit expression (`clippy::unused_unit`).
    let plugins_fn = if plugin_values.is_empty() {
        quote! {}
    } else {
        quote! { ( #(#plugin_values,)* ) }
    };

    // `prefix = "/api/v1"` → the module's HTTP mount point. Omitted (the
    // common case) leaves the trait's `None` default in place.
    let path_prefix = match &args.prefix {
        Some(prefix) => quote! { const PATH_PREFIX: Option<&'static str> = Some(#prefix); },
        None => quote! {},
    };

    quote! {
        #item

        impl #krate::di::module::FeatureModule for #name {
            #path_prefix
            type Providers = #providers;
            type Controllers = #controllers;
            type Exports = #exports;
            type Imports = #imports;
            type RequiredPlugins = #required_plugins;
            type Plugins = #plugins_ty;
            type Endpoints = #endpoints;

            fn plugins() -> Self::Plugins {
                #plugins_fn
            }
        }
    }
    .into()
}
