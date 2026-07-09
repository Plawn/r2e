//! `#[derive(BackgroundService)]` — generates `impl ServiceComponent<State>`.
//!
//! Mirrors the field resolution of `#[controller(...)]` (struct-level
//! identity is not supported here — background services have no request
//! context). The user implements an async `run(&self, CancellationToken)`
//! method on the struct; the derive wires `from_state` + `start`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::crate_path::r2e_core_path;
use crate::field_resolver::{
    ClassifyOpts, FieldKind, classify_fields, config_init_panic, config_section_init_panic,
};

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
    // with a migration hint.
    for attr in &input.attrs {
        if attr.path().is_ident("service") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("state") {
                    Err(meta.error(
                        "`#[service(state = ...)]` was removed — background services are \
                         constructed from the bean graph by type; drop the attribute and make \
                         sure every #[inject] field type is provided/registered on the AppBuilder",
                    ))
                } else {
                    Err(meta.error(
                        "unknown attribute in #[service(...)]",
                    ))
                }
            })?;
        }
    }

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            Fields::Unit => {
                return Ok(generate_unit_impl(name, &krate));
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
            allow_named_inject: false,
            allow_default: false,
            context_label: "background service",
        },
    )?;

    let mut field_inits: Vec<TokenStream2> = Vec::new();
    let mut has_any_config = false;

    for cf in &classified {
        match &cf.kind {
            FieldKind::Inject => {
                let field_name = cf.name;
                let field_ty = cf.ty;
                field_inits.push(quote! { #field_name: __ctx.get::<#field_ty>() });
            }
            FieldKind::ConfigSection { prefix } => {
                field_inits.push(config_section_init_panic(
                    cf.name, cf.ty, prefix, &name_str, &krate,
                ));
                has_any_config = true;
            }
            FieldKind::Config { key, env_hint, .. } => {
                field_inits.push(config_init_panic(cf.name, key, env_hint, &name_str));
                has_any_config = true;
            }
            FieldKind::InjectNamed { .. } | FieldKind::Default => unreachable!(),
        }
    }

    let config_prelude = if has_any_config {
        quote! {
            let __cfg = __ctx.get::<#krate::R2eConfig>();
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        impl #krate::ServiceComponent for #name {
            fn from_context(__ctx: &#krate::beans::BeanContext) -> Self {
                #config_prelude
                Self {
                    #(#field_inits,)*
                }
            }

            fn start(
                self,
                __shutdown: ::tokio_util::sync::CancellationToken,
            ) -> impl ::core::future::Future<Output = ()> + Send {
                async move { self.run(__shutdown).await }
            }
        }
    })
}

fn generate_unit_impl(
    name: &syn::Ident,
    krate: &TokenStream2,
) -> TokenStream2 {
    quote! {
        impl #krate::ServiceComponent for #name {
            fn from_context(_ctx: &#krate::beans::BeanContext) -> Self { #name }

            fn start(
                self,
                __shutdown: ::tokio_util::sync::CancellationToken,
            ) -> impl ::core::future::Future<Output = ()> + Send {
                async move { self.run(__shutdown).await }
            }
        }
    }
}
