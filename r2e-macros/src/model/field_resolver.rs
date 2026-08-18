use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;

use crate::util::type_utils::{
    config_hint_sentence, parse_config_field, parse_config_section_prefix, parse_inject_name,
    parse_live_config_field,
};

pub enum FieldKind {
    Inject,
    InjectNamed { name: String },
    Config { key: String, ty_name: String },
    /// `#[live_config("key")]` — a runtime-updatable `LiveConfig<T>` handle
    /// resolved from the `LiveConfigRegistry` bean. App-scoped like `#[config]`
    /// (resolved once at construction); the *value* is what changes at runtime.
    LiveConfig { key: String, ty_name: String },
    ConfigSection { prefix: String },
    Default,
}

pub struct ClassifiedField<'a> {
    pub name: &'a syn::Ident,
    pub ty: &'a syn::Type,
    pub kind: FieldKind,
}

pub struct ClassifyOpts {
    pub allow_named_inject: bool,
    pub allow_default: bool,
    pub context_label: &'static str,
}

pub fn classify_fields<'a>(
    fields: impl Iterator<Item = &'a syn::Field>,
    opts: &ClassifyOpts,
) -> syn::Result<Vec<ClassifiedField<'a>>> {
    let mut result = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_type = &field.ty;

        let is_inject = field.attrs.iter().any(|a| a.path().is_ident("inject"));
        let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));
        let config_section_attr = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("config_section"));
        let live_config_attr = field.attrs.iter().find(|a| a.path().is_ident("live_config"));
        let is_default = field.attrs.iter().any(|a| a.path().is_ident("default"));

        check_live_config_exclusive(&field.attrs)?;

        if let Some(attr) = live_config_attr {
            let (key, ty_name) = parse_live_config_field(attr, field_type)?;
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind: FieldKind::LiveConfig { key, ty_name },
            });
        } else if is_inject {
            let named = if opts.allow_named_inject {
                parse_inject_name(&field.attrs)?
            } else {
                None
            };
            let kind = match named {
                Some(name) => FieldKind::InjectNamed { name },
                None => FieldKind::Inject,
            };
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind,
            });
        } else if let Some(attr) = config_section_attr {
            let prefix = parse_config_section_prefix(attr)?;
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind: FieldKind::ConfigSection { prefix },
            });
        } else if let Some(attr) = config_attr {
            let (key, ty_name) = parse_config_field(attr, field_type)?;
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind: FieldKind::Config { key, ty_name },
            });
        } else if is_default && opts.allow_default {
            result.push(ClassifiedField {
                name: field_name,
                ty: field_type,
                kind: FieldKind::Default,
            });
        } else {
            let mut hints = vec!["#[inject]                           — clone from app state"];
            if opts.allow_named_inject {
                hints.push("#[inject(name = \"...\")]             — named injection via newtype");
            }
            hints.push("#[config(\"app.key\")]                — resolve from R2eConfig");
            hints.push(
                "#[live_config(\"app.key\")]           — runtime-updatable LiveConfig<T> handle",
            );
            hints.push("#[config_section(prefix = \"app\")]   — resolve a typed config section");
            if opts.allow_default {
                hints.push("#[default]                          — use `Default::default()`");
            }
            let msg = format!(
                "{} field must be annotated with one of:\n{}",
                opts.context_label,
                hints.iter().map(|h| format!("\n  {h}")).collect::<String>()
            );
            return Err(syn::Error::new_spanned(field_name, msg));
        }
    }

    Ok(result)
}

/// Reject `#[live_config]` combined with another injection scope on the same
/// field or parameter.
///
/// `#[live_config]` names its own key and produces its own handle type, so
/// stacking it on an `#[inject]` / `#[config]` / `#[config_section]` declaration
/// is always a mistake — the two would fight over the same slot. Shared by every
/// host (controllers, `#[derive(Bean)]`, decorator beans, background services,
/// `#[bean]` / `#[producer]` parameters) so the diagnostic is identical
/// everywhere.
pub fn check_live_config_exclusive(attrs: &[syn::Attribute]) -> syn::Result<()> {
    let Some(live) = attrs.iter().find(|a| a.path().is_ident("live_config")) else {
        return Ok(());
    };

    if attrs.iter().any(|a| a.path().is_ident("inject")) {
        return Err(syn::Error::new_spanned(
            live,
            "#[live_config] cannot be combined with #[inject] — pick one scope:\n\
             \n  #[inject]                  — clone an app-scoped bean\
             \n  #[live_config(\"app.key\")]  — runtime-updatable config handle (LiveConfig<T>)",
        ));
    }
    if attrs.iter().any(|a| a.path().is_ident("config")) {
        return Err(syn::Error::new_spanned(
            live,
            "#[live_config] cannot be combined with #[config] — pick one:\n\
             \n  #[config(\"app.key\")]       — boot-time snapshot (plain T)\
             \n  #[live_config(\"app.key\")]  — runtime-updatable handle (LiveConfig<T>)",
        ));
    }
    if attrs.iter().any(|a| a.path().is_ident("config_section")) {
        return Err(syn::Error::new_spanned(
            live,
            "#[live_config] cannot be combined with #[config_section] — pick one:\n\
             \n  #[config_section(prefix = \"app\")]  — boot-time typed section\
             \n  #[live_config(\"app.key\")]          — runtime-updatable handle (LiveConfig<T>)",
        ));
    }
    Ok(())
}

/// Produce the resolution **expression** for a single `#[live_config("key")]`
/// field or parameter: a typed handle pulled from the `LiveConfigRegistry` bean.
///
/// The single shared source of `#[live_config]` init codegen (controllers,
/// beans, decorator beans, background services, producers). `registry` is the
/// registry receiver expression (e.g. `__r2e_live`); the expression is spanned
/// at the declared type so a field that is not a `LiveConfig<T>` reports the
/// mismatch on the field, not on the macro call site.
pub fn live_config_resolve_expr(
    registry: &TokenStream2,
    key: &str,
    ty: Option<&syn::Type>,
) -> TokenStream2 {
    let span = ty.map_or_else(proc_macro2::Span::call_site, |ty| ty.span());
    quote_spanned! { span => #registry.live_config(#key) }
}

/// The `LiveConfigRegistry` bean type path — the dependency every
/// `#[live_config]` host gains (so a build without `load_config` fails with the
/// standard missing-bean diagnostics).
pub fn live_config_registry_ty(krate: &TokenStream2) -> TokenStream2 {
    quote! { #krate::config::LiveConfigRegistry }
}

/// The `config_keys()` return type every host emits — `(key, type name, kind)`.
///
/// The kind is what tells the runtime which of R2E's two config freshness modes
/// a key uses: `Required`/`Optional` are **copied** (fingerprinted, so an edit
/// rebuilds the declaring bean), `Live` is **subscribed** (pushed through the
/// registry, so it must stay out of the fingerprint).
pub fn config_keys_ret_ty(krate: &TokenStream2) -> TokenStream2 {
    quote! { Vec<(&'static str, &'static str, #krate::config::ConfigKeyKind)> }
}

/// A `config_keys()` entry for a **copied** `#[config("key")]` field/param.
/// `Option<T>` fields are `Optional` (not presence-validated), everything else
/// is `Required`. Both are fingerprinted.
pub fn copied_config_key_entry(
    krate: &TokenStream2,
    key: &str,
    ty_name: &str,
    is_option: bool,
) -> TokenStream2 {
    let kind = if is_option {
        quote! { #krate::config::ConfigKeyKind::Optional }
    } else {
        quote! { #krate::config::ConfigKeyKind::Required }
    };
    quote! { (#key, #ty_name, #kind) }
}

/// A `config_keys()` entry for a **copied** `#[config_section(prefix = "…")]`
/// field/param.
///
/// The entry's key is the **prefix**, tagged `Section` so the runtime hashes
/// every config key under it instead of looking up one exact key: the section's
/// field set lives in the typed `ConfigProperties` struct, not in the attribute,
/// so nothing here can enumerate it. Without this entry a bean holding a typed
/// section keeps a stable fingerprint when a key inside the section is edited,
/// and `r2e dev` reuses it with the stale struct.
///
/// Not presence-validated (`is_required()` is false for `Section`): the section
/// validates itself at construction — `ConfigProperties::from_config` fails and
/// the generated init panics naming the prefix. Hosts that construct *late*
/// (decorator beans, background services) pair this with a
/// [`section_validator_entry`] so they can validate at registration instead.
pub fn section_config_key_entry(krate: &TokenStream2, prefix: &str, ty: &syn::Type) -> TokenStream2 {
    let ty_name = quote!(#ty).to_string();
    quote! { (#prefix, #ty_name, #krate::config::ConfigKeyKind::Section) }
}

/// The `config_sections()` return type — type-aware section validators, for
/// hosts whose construction happens too late for "it validates itself at
/// construction" to count as startup validation.
pub fn config_sections_ret_ty(krate: &TokenStream2) -> TokenStream2 {
    quote! { Vec<#krate::config::SectionValidator> }
}

/// A `config_sections()` entry for a `#[config_section(prefix = "…")]` field.
///
/// Carries the section **type**, not just its name, so the host can run the
/// same `validate_section::<Ty>` walk a controller field gets — missing keys,
/// nested sections, type mismatches, garde violations.
pub fn section_validator_entry(krate: &TokenStream2, prefix: &str, ty: &syn::Type) -> TokenStream2 {
    quote! { #krate::config::SectionValidator::of::<#ty>(#prefix) }
}

/// A `config_keys()` entry for a **subscribed** `#[live_config("key")]`
/// field/param: never presence-validated (the value may legitimately be absent
/// at boot — the handle's `get()` returns a `Result`) and never fingerprinted
/// (freshness arrives by push, so a rebuild would be pointless churn).
pub fn live_config_key_entry(krate: &TokenStream2, key: &str, ty_name: &str) -> TokenStream2 {
    quote! { (#key, #ty_name, #krate::config::ConfigKeyKind::Live) }
}

/// The `__r2e_live` prelude binding hosts emit once when at least one
/// `#[live_config]` field/param is present, and nothing at all otherwise.
///
/// `ctx` is the `BeanContext` receiver expression (`ctx` / `__ctx`). The
/// `present` gate lives here rather than at each of the six call sites: the
/// binding and the condition that justifies it are one decision.
pub fn live_config_prelude(
    ctx: &TokenStream2,
    krate: &TokenStream2,
    present: bool,
) -> TokenStream2 {
    if !present {
        return quote! {};
    }
    let ty = live_config_registry_ty(krate);
    quote! { let __r2e_live: #ty = #ctx.get::<#ty>(); }
}

/// Produce the config-resolution **expression** for a single `#[config]` field
/// or param. This is the single shared source of `#[config]` init codegen, used
/// by controllers, beans, producers, decorator beans, and background services.
/// The caller binds the returned expression (as a `let` statement or a
/// struct-literal field).
///
/// - `cfg`: the config receiver expression (e.g. `__cfg` or `__r2e_config`).
/// - `key`: the config key.
/// - `ty`: `Some(ty)` emits a turbofish `get::<ty>(...)`; `None` lets the
///   binding site infer the type (struct-literal fields).
/// - `owner`: human label for panics (e.g. `` bean `Foo` `` or `` `UserController` ``).
/// - `is_option`: an `Option<T>` field resolves an absent key (or explicit
///   `null`) to `None`; a type mismatch still panics — with the same actionable
///   hint as a required key.
pub fn config_resolve_expr(
    cfg: &TokenStream2,
    key: &str,
    ty: Option<&syn::Type>,
    owner: &str,
    is_option: bool,
    krate: &TokenStream2,
) -> TokenStream2 {
    let hint = config_hint_sentence(key);
    let getter = match ty {
        Some(ty) => quote! { #cfg.get::<#ty>(#key) },
        None => quote! { #cfg.get(#key) },
    };
    if is_option {
        // `Option<T>` config fields are optional: an absent key maps to `None`
        // (explicit `null` too, via `FromConfigValue for Option<T>`). A
        // present-but-mistyped value still panics — with the hint (Fix 5).
        quote! {
            match #getter {
                Ok(__v) => __v,
                Err(#krate::config::ConfigError::NotFound(_)) => None,
                Err(__e) => panic!(
                    "Configuration error in {}: key '{}' — {}. {}",
                    #owner, #key, __e, #hint
                ),
            }
        }
    } else {
        quote! {
            #getter.unwrap_or_else(|__e| panic!(
                "Configuration error in {}: key '{}' — {}. {}",
                #owner, #key, __e, #hint
            ))
        }
    }
}

/// Struct-literal `#[config]` field init (`#field_name: <expr>`), for owners
/// that build the target via a struct literal with inferred field types
/// (controllers, background services). Delegates to [`config_resolve_expr`].
pub fn config_init_panic(
    field_name: &syn::Ident,
    key: &str,
    owner_name: &str,
    is_option: bool,
    krate: &TokenStream2,
) -> TokenStream2 {
    let owner = format!("`{owner_name}`");
    let expr = config_resolve_expr(&quote! { __cfg }, key, None, &owner, is_option, krate);
    quote! { #field_name: #expr }
}

/// Produce the resolution **expression** for a single
/// `#[config_section(prefix = "…")]` field or parameter — the section
/// counterpart of [`config_resolve_expr`], and the single shared source of
/// `#[config_section]` init codegen across every host.
///
/// - `cfg`: the config receiver expression (e.g. `__cfg` or `__r2e_config`).
/// - `ty`: the declared type, used for the `ConfigProperties` qualification so
///   a type that does not implement it reports on the field, not the call site.
/// - `owner`: human label for the panic (e.g. `` bean `Foo` ``), same
///   convention as [`config_resolve_expr`], so a config failure reads the same
///   whether it came from a key or a section.
pub fn config_section_resolve_expr(
    cfg: &TokenStream2,
    prefix: &str,
    ty: &syn::Type,
    krate: &TokenStream2,
    owner: &str,
) -> TokenStream2 {
    quote_spanned! { ty.span() =>
        <#ty as #krate::config::ConfigProperties>::from_config(&#cfg, Some(#prefix))
            .unwrap_or_else(|__e| panic!(
                "Configuration error in {}: config section '{}' — {}",
                #owner, #prefix, __e,
            ))
    }
}

/// Struct-literal `#[config_section]` field init (`#field_name: <expr>`), for
/// owners that build the target via a struct literal (controllers, background
/// services). Delegates to [`config_section_resolve_expr`].
pub fn config_section_init_panic(
    field_name: &syn::Ident,
    field_type: &syn::Type,
    prefix: &str,
    owner_name: &str,
    krate: &TokenStream2,
) -> TokenStream2 {
    let owner = format!("`{owner_name}`");
    let expr = config_section_resolve_expr(&quote! { __cfg }, prefix, field_type, krate, &owner);
    quote! { #field_name: #expr }
}
