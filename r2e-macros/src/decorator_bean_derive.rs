//! `#[derive(DecoratorBean)]` — a guard/interceptor with bean deps, without
//! the spec/product boilerplate.
//!
//! The derived struct is the **product** (the type implementing `Guard<I>` /
//! `Interceptor<R>`). Fields are split by attribute:
//!
//! - `#[inject]` — resolved from the bean graph at wiring time (also
//!   `#[inject(name = "...")]` for newtype-named beans);
//! - `#[config("key")]` / `#[config_section(prefix = "...")]` — resolved
//!   from `R2eConfig`;
//! - plain fields — **config, set at the attribute site** through a
//!   generated associated constructor `spec(...)` taking them in
//!   declaration order.
//!
//! ```ignore
//! #[derive(DecoratorBean)]
//! pub struct DbAuditLog {
//!     #[inject] pool: SqlitePool,
//!     prefix: String,
//! }
//!
//! impl<R: Send> Interceptor<R> for DbAuditLog { ... }
//!
//! // at the site:
//! #[intercept(DbAuditLog::spec("api".into()))]
//! ```
//!
//! Generated items:
//!
//! - a hidden companion spec `__R2eSpec_<Name>` holding the plain fields,
//!   returned by `<Name>::spec(...)`;
//! - `impl DecoratorSpec for __R2eSpec_<Name>` — the real build: injected
//!   fields from the context, config fields from `R2eConfig`, plain fields
//!   from the spec value;
//! - `impl DecoratorSpec for <Name>` — an identity impl carrying the same
//!   `Deps`. `#[routes]` extracts `<Name>` from the expression's leading
//!   path and folds `<Name as DecoratorSpec>::Deps` into the controller's
//!   dep list; `build_decorator`'s equality bounds tie the two impls
//!   together. It also makes the `#[guard(Name = <prebuilt value>)]` escape
//!   hatch work with an already-constructed product.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::crate_path::r2e_core_path;
use crate::field_resolver::{classify_fields, ClassifyOpts, FieldKind};
use crate::type_list_gen::build_tcons_type;
use crate::type_utils::named_bean_newtype_ident;

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
    let vis = &input.vis;

    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "#[derive(DecoratorBean)] does not support generic types",
        ));
    }

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(DecoratorBean)] only works on structs with named fields:\n\
                     \n  #[derive(DecoratorBean)]\n  struct MyGuard {\n      #[inject] pool: PgPool,\n      max: u64, // plain field = config, set via MyGuard::spec(max)\n  }",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(DecoratorBean)] only works on structs — enums and unions are not supported",
            ))
        }
    };

    // Plain fields are config-by-constructor; only attributed fields go
    // through the shared classifier (which rejects unannotated fields).
    let is_resolved = |f: &syn::Field| {
        f.attrs.iter().any(|a| {
            a.path().is_ident("inject")
                || a.path().is_ident("config")
                || a.path().is_ident("config_section")
        })
    };
    let (resolved, plain): (Vec<&syn::Field>, Vec<&syn::Field>) =
        fields.iter().partition(|f| is_resolved(f));

    let krate = r2e_core_path();
    let classified = classify_fields(
        resolved.into_iter(),
        &ClassifyOpts {
            allow_named_inject: true,
            allow_default: false,
            context_label: "decorator bean",
        },
    )?;

    let mut dep_types: Vec<TokenStream2> = Vec::new();
    let mut resolved_inits: Vec<TokenStream2> = Vec::new();
    let mut has_config = false;

    for cf in &classified {
        let field_name = cf.name;
        let field_type = cf.ty;

        match &cf.kind {
            FieldKind::InjectNamed { name } => {
                let newtype_ident = named_bean_newtype_ident(name, field_type);
                dep_types.push(quote! { #newtype_ident });
                resolved_inits.push(quote! { #field_name: __ctx.get::<#newtype_ident>().0 });
            }
            FieldKind::Inject => {
                dep_types.push(quote! { #field_type });
                resolved_inits.push(quote! { #field_name: __ctx.get::<#field_type>() });
            }
            FieldKind::ConfigSection { prefix } => {
                resolved_inits.push(quote! {
                    #field_name: #krate::config::ConfigProperties::from_config(&__cfg, Some(#prefix)).unwrap_or_else(|e| {
                        panic!(
                            "Configuration error in decorator bean `{}`: config section '{}' — {}",
                            #name_str, #prefix, e
                        )
                    })
                });
                has_config = true;
            }
            FieldKind::Config { key, env_hint, .. } => {
                resolved_inits.push(quote! {
                    #field_name: __cfg.get::<#field_type>(#key).unwrap_or_else(|_| {
                        panic!(
                            "Configuration error in decorator bean `{}`: key '{}' — Config key not found. \
                             Add it to application.yaml or set env var `{}`.",
                            #name_str, #key, #env_hint
                        )
                    })
                });
                has_config = true;
            }
            FieldKind::Default => unreachable!("allow_default is false"),
        }
    }

    if has_config {
        dep_types.push(quote! { #krate::config::R2eConfig });
    }
    let deps_type = build_tcons_type(&dep_types, &krate);

    let config_prelude = if has_config {
        quote! { let __cfg: #krate::config::R2eConfig = __ctx.get::<#krate::config::R2eConfig>(); }
    } else {
        quote! {}
    };

    let spec_ident = format_ident!("__R2eSpec_{}", name);
    let plain_names: Vec<&syn::Ident> = plain.iter().map(|f| f.ident.as_ref().unwrap()).collect();
    let plain_types: Vec<&syn::Type> = plain.iter().map(|f| &f.ty).collect();

    let spec_doc = format!(
        "Config spec for [`{name_str}`] — the value `#[guard(...)]` / \
         `#[intercept(...)]` sites build the decorator from. Constructed via \
         [`{name_str}::spec`]."
    );
    let ctor_doc = format!(
        "Build the config spec for a `#[guard({name_str}::spec(...))]` / \
         `#[intercept({name_str}::spec(...))]` site. Takes the non-injected \
         fields in declaration order; `#[inject]`/`#[config]` fields are \
         resolved from the bean graph at controller registration."
    );

    Ok(quote! {
        #[doc = #spec_doc]
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #vis struct #spec_ident {
            #(#plain_names: #plain_types,)*
        }

        impl #name {
            #[doc = #ctor_doc]
            #vis fn spec(#(#plain_names: #plain_types),*) -> #spec_ident {
                #spec_ident { #(#plain_names,)* }
            }
        }

        impl #krate::DecoratorSpec for #spec_ident {
            type Product = #name;
            type Deps = #deps_type;

            fn build(self, __ctx: &#krate::beans::BeanContext) -> #name {
                #config_prelude
                #name {
                    #(#resolved_inits,)*
                    #(#plain_names: self.#plain_names,)*
                }
            }
        }

        // Deps carrier for the leading type path extracted by `#[routes]`
        // (`<#name as DecoratorSpec>::Deps` is what the controller folds),
        // and identity build for the `#[guard(#name = <value>)]` escape
        // hatch. `build_decorator`'s equality bounds keep it in lockstep
        // with the companion spec above.
        impl #krate::DecoratorSpec for #name {
            type Product = #name;
            type Deps = #deps_type;

            fn build(self, __ctx: &#krate::beans::BeanContext) -> #name {
                self
            }
        }
    })
}
