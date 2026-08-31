//! `#[derive(BackgroundService)]` — generates `impl ServiceComponent`.
//!
//! Mirrors the field resolution of `#[controller(...)]` (struct-level
//! identity is not supported here — background services have no request
//! context). The user implements an async `run(&self, rt::CancelToken)`
//! method on the struct; the derive wires `type Deps` + `config_keys` +
//! `from_context` + `start`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::model::field_resolver::{
    classify_fields, config_init_panic, config_section_init_panic, ClassifyOpts, FieldKind,
};
use crate::util::crate_path::r2e_core_path;

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate(&input) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn generate(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let name_str = name.to_string();
    let krate = r2e_core_path();

    // Phase 4: services construct from the bean graph by type — a named state
    // no longer exists. Reject the removed `#[service(state = ...)]` attribute
    // with a migration hint. `enabled = "…"` is the one accepted argument.
    let mut enabled_gate: Option<syn::LitStr> = None;
    for attr in &input.attrs {
        if attr.path().is_ident("service") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("state") {
                    Err(meta.error(
                        "`#[service(state = ...)]` was removed — background services are \
                         constructed from the bean graph by type; drop the attribute and make \
                         sure every #[inject] field type is provided/registered on the AppBuilder",
                    ))
                } else if meta.path.is_ident("enabled") {
                    if enabled_gate.is_some() {
                        return Err(meta.error("duplicate `enabled` in #[service(...)]"));
                    }
                    enabled_gate = Some(meta.value()?.parse::<syn::LitStr>()?);
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown attribute in #[service(...)] — expected `enabled = \"<field or \
                         method>\"`",
                    ))
                }
            })?;
        }
    }

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            Fields::Unit => {
                return generate_unit_impl(name, &krate, enabled_gate.as_ref());
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(BackgroundService)] requires named fields or a unit struct",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(BackgroundService)] only works on structs",
            ))
        }
    };

    let classified = classify_fields(
        fields.iter(),
        &ClassifyOpts {
            allow_default: false,
            context_label: "background service",
        },
    )?;

    let mut field_inits: Vec<TokenStream2> = Vec::new();
    // The bean types `from_context` pulls — folded into `ServiceComponent::Deps`
    // so `spawn_service` rejects a service reading an absent bean at compile
    // time instead of panicking in `ctx.get()`.
    let mut dep_types: Vec<TokenStream2> = Vec::new();
    // Declared config keys — presence-validated at `spawn_service` (and at graph
    // resolution for `#[producer(start)]` services). NOT fingerprinted: a
    // background service is started, never reused across dev-reload cycles, so
    // it has no per-registration fingerprint to participate in.
    let mut config_key_entries: Vec<TokenStream2> = Vec::new();
    // Type-aware `#[config_section]` declarations. A service constructs when its
    // task starts, so "the section validates itself at construction" would mean
    // "it panics at serve time" — these let `spawn_service` / graph resolution
    // run the real section walk up front.
    let mut config_section_entries: Vec<TokenStream2> = Vec::new();
    let mut has_any_config = false;
    let mut has_live_config = false;

    for cf in &classified {
        match &cf.kind {
            FieldKind::Inject => {
                let field_name = cf.name;
                let field_ty = cf.ty;
                dep_types.push(quote! { #field_ty });
                field_inits.push(quote! { #field_name: __ctx.get::<#field_ty>() });
            }
            FieldKind::ConfigSection { prefix } => {
                config_key_entries.push(crate::model::field_resolver::section_config_key_entry(
                    &krate, prefix, cf.ty,
                ));
                config_section_entries.push(crate::model::field_resolver::section_validator_entry(
                    &krate, prefix, cf.ty,
                ));
                field_inits.push(config_section_init_panic(
                    cf.name, cf.ty, prefix, &name_str, &krate,
                ));
                has_any_config = true;
            }
            FieldKind::Config { key, ty_name } => {
                let is_option = crate::util::type_utils::is_option_type(cf.ty);
                config_key_entries.push(crate::model::field_resolver::copied_config_key_entry(
                    &krate, key, ty_name, is_option,
                ));
                field_inits.push(config_init_panic(
                    cf.name, key, &name_str, is_option, &krate,
                ));
                has_any_config = true;
            }
            FieldKind::LiveConfig { key, ty_name } => {
                let field_name = cf.name;
                config_key_entries.push(crate::model::field_resolver::live_config_key_entry(
                    &krate, key, ty_name,
                ));
                let expr = crate::model::field_resolver::live_config_resolve_expr(
                    &quote! { __r2e_live },
                    key,
                    Some(cf.ty),
                );
                field_inits.push(quote! { #field_name: #expr });
                has_live_config = true;
            }
            FieldKind::Default => unreachable!(),
        }
    }

    if has_any_config {
        dep_types.push(quote! { #krate::config::R2eConfig });
    }
    if has_live_config {
        dep_types.push(crate::model::field_resolver::live_config_registry_ty(
            &krate,
        ));
    }
    let deps_type = crate::model::type_list_gen::build_tcons_type(&dep_types, &krate);

    let config_keys_ret_ty = crate::model::field_resolver::config_keys_ret_ty(&krate);
    let config_keys_fn = if config_key_entries.is_empty() {
        quote! {}
    } else {
        quote! {
            fn config_keys() -> #config_keys_ret_ty {
                vec![#(#config_key_entries),*]
            }
        }
    };

    let config_sections_ret_ty = crate::model::field_resolver::config_sections_ret_ty(&krate);
    let config_sections_fn = if config_section_entries.is_empty() {
        quote! {}
    } else {
        quote! {
            fn config_sections() -> #config_sections_ret_ty {
                vec![#(#config_section_entries),*]
            }
        }
    };

    // `#[service(enabled = "name")]`: `name` is resolved against the struct's
    // own fields first (the common case — a `#[config("…")] enabled: bool`
    // flag, whose config key then becomes the logged gate label), and read as a
    // `&self` method otherwise. Nothing else can name a `bool` here, so the two
    // forms cannot be confused with each other.
    let enabled_fns = match &enabled_gate {
        None => quote! {},
        Some(lit) => {
            let raw = lit.value();
            let mut ident: syn::Ident = syn::parse_str(&raw).map_err(|_| {
                syn::Error::new_spanned(
                    lit,
                    "#[service(enabled = \"…\")] expects the name of a `bool` field or of a \
                     `&self` method returning `bool`",
                )
            })?;
            ident.set_span(lit.span());
            let field = classified.iter().find(|cf| *cf.name == ident);
            let (expr, label) = match field {
                Some(cf) => {
                    let label = match &cf.kind {
                        // A config-backed flag: name the KEY, which is the
                        // switch an operator actually flips.
                        FieldKind::Config { key, .. } | FieldKind::LiveConfig { key, .. } => {
                            key.clone()
                        }
                        _ => raw.clone(),
                    };
                    (quote! { self.#ident }, label)
                }
                None => (quote! { self.#ident() }, format!("{raw}()")),
            };
            quote! {
                fn enabled(&self) -> bool { #expr }

                fn enabled_gate() -> ::core::option::Option<&'static str> {
                    ::core::option::Option::Some(#label)
                }
            }
        }
    };

    let config_prelude = if has_any_config {
        quote! {
            let __cfg = __ctx.get::<#krate::R2eConfig>();
        }
    } else {
        quote! {}
    };
    let live_config_prelude = crate::model::field_resolver::live_config_prelude(
        &quote! { __ctx },
        &krate,
        has_live_config,
    );

    Ok(quote! {
        impl #krate::ServiceComponent for #name {
            type Deps = #deps_type;

            #config_keys_fn

            #config_sections_fn

            #enabled_fns

            fn from_context(__ctx: &#krate::beans::BeanContext) -> Self {
                #config_prelude
                #live_config_prelude
                Self {
                    #(#field_inits,)*
                }
            }

            fn start(
                self,
                __shutdown: #krate::rt::CancelToken,
            ) -> impl ::core::future::Future<Output = ()> + Send {
                async move { self.run(__shutdown).await }
            }
        }
    })
}

fn generate_unit_impl(
    name: &syn::Ident,
    krate: &TokenStream2,
    enabled_gate: Option<&syn::LitStr>,
) -> syn::Result<TokenStream2> {
    // A unit struct has no fields, so the gate can only be a `&self` method.
    let enabled_fns = match enabled_gate {
        None => quote! {},
        Some(lit) => {
            let raw = lit.value();
            let mut ident: syn::Ident = syn::parse_str(&raw).map_err(|_| {
                syn::Error::new_spanned(
                    lit,
                    "#[service(enabled = \"…\")] on a unit struct expects the name of a `&self` \
                     method returning `bool`",
                )
            })?;
            ident.set_span(lit.span());
            let label = format!("{raw}()");
            quote! {
                fn enabled(&self) -> bool { self.#ident() }

                fn enabled_gate() -> ::core::option::Option<&'static str> {
                    ::core::option::Option::Some(#label)
                }
            }
        }
    };

    Ok(quote! {
        impl #krate::ServiceComponent for #name {
            type Deps = #krate::type_list::TNil;

            #enabled_fns

            fn from_context(_ctx: &#krate::beans::BeanContext) -> Self { #name }

            fn start(
                self,
                __shutdown: #krate::rt::CancelToken,
            ) -> impl ::core::future::Future<Output = ()> + Send {
                async move { self.run(__shutdown).await }
            }
        }
    })
}
