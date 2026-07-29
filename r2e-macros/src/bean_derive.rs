use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

use crate::crate_path::r2e_core_path;
use crate::field_resolver::{classify_fields, ClassifyOpts, FieldKind};
use crate::hash_tokens::hash_token_stream;
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

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "#[derive(Bean)] only works on structs with named fields:\n\
                     \n  #[derive(Bean, Clone)]\n  struct MyService {\n      #[inject] dep: OtherService,\n  }",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(Bean)] only works on structs — enums and unions are not supported",
            ))
        }
    };

    let krate = r2e_core_path();
    let classified = classify_fields(
        fields.iter(),
        &ClassifyOpts {
            allow_named_inject: true,
            allow_default: true,
            context_label: "bean",
        },
    )?;

    let mut dep_type_ids = Vec::new();
    let mut dep_types: Vec<TokenStream2> = Vec::new();
    let mut field_inits = Vec::new();
    let mut config_key_entries = Vec::new();
    let mut has_config = false;
    let mut has_live_config = false;

    for cf in &classified {
        let field_name = cf.name;
        let field_type = cf.ty;

        match &cf.kind {
            FieldKind::InjectNamed { name } => {
                let newtype_ident = named_bean_newtype_ident(name, field_type);
                dep_type_ids.push(quote! { (std::any::TypeId::of::<#newtype_ident>(), std::any::type_name::<#newtype_ident>()) });
                dep_types.push(quote! { #newtype_ident });
                field_inits.push(quote! { #field_name: ctx.get::<#newtype_ident>().0 });
            }
            FieldKind::Inject => {
                dep_type_ids.push(quote! { (std::any::TypeId::of::<#field_type>(), std::any::type_name::<#field_type>()) });
                dep_types.push(quote! { #field_type });
                field_inits.push(quote! { #field_name: ctx.get::<#field_type>() });
            }
            FieldKind::ConfigSection { prefix } => {
                // Declared as `Section`: the key is the PREFIX, so dev-reload
                // fingerprints every config key underneath it and a change
                // anywhere in the section rebuilds this bean.
                config_key_entries.push(crate::field_resolver::section_config_key_entry(
                    &krate, prefix, field_type,
                ));
                let owner = format!("bean `{name_str}`");
                let expr = crate::field_resolver::config_section_resolve_expr(
                    &quote! { __cfg },
                    prefix,
                    field_type,
                    &krate,
                    &owner,
                );
                field_inits.push(quote! { #field_name: #expr });
                has_config = true;
            }
            FieldKind::Config { key, ty_name } => {
                let is_option = crate::type_utils::is_option_type(field_type);
                // Emit a `config_keys()` entry for EVERY copied key (required
                // and optional) so dev-reload fingerprints the value; the kind
                // gates presence validation.
                config_key_entries.push(crate::field_resolver::copied_config_key_entry(
                    &krate, key, ty_name, is_option,
                ));
                let owner = format!("bean `{name_str}`");
                let expr = crate::field_resolver::config_resolve_expr(
                    &quote! { __cfg },
                    key,
                    Some(field_type),
                    &owner,
                    is_option,
                    &krate,
                );
                field_inits.push(quote! { #field_name: #expr });
                has_config = true;
            }
            FieldKind::LiveConfig { key, ty_name } => {
                // Declared as `Live`: never presence-validated and never
                // fingerprinted — the value is pushed through the registry
                // slot, so an edit must NOT rebuild the bean.
                config_key_entries.push(crate::field_resolver::live_config_key_entry(
                    &krate, key, ty_name,
                ));
                let expr = crate::field_resolver::live_config_resolve_expr(
                    &quote! { __r2e_live },
                    key,
                    Some(field_type),
                );
                field_inits.push(quote! { #field_name: #expr });
                has_live_config = true;
            }
            FieldKind::Default => {
                field_inits.push(quote! { #field_name: ::core::default::Default::default() });
            }
        }
    }

    if has_config {
        dep_type_ids.push(
            quote! { (std::any::TypeId::of::<#krate::config::R2eConfig>(), std::any::type_name::<#krate::config::R2eConfig>()) },
        );
        dep_types.push(quote! { #krate::config::R2eConfig });
    }
    if has_live_config {
        let live_ty = crate::field_resolver::live_config_registry_ty(&krate);
        dep_type_ids.push(
            quote! { (std::any::TypeId::of::<#live_ty>(), std::any::type_name::<#live_ty>()) },
        );
        dep_types.push(live_ty);
    }

    let deps_type = build_tcons_type(&dep_types, &krate);
    let build_version = hash_token_stream(&quote! { #fields });

    let config_prelude = if has_config {
        quote! { let __cfg: #krate::config::R2eConfig = ctx.get::<#krate::config::R2eConfig>(); }
    } else {
        quote! {}
    };
    let live_config_prelude =
        crate::field_resolver::live_config_prelude(&quote! { ctx }, &krate, has_live_config);

    let config_keys_ret_ty = crate::field_resolver::config_keys_ret_ty(&krate);
    let config_keys_fn = if config_key_entries.is_empty() {
        quote! {}
    } else {
        quote! {
            fn config_keys() -> #config_keys_ret_ty {
                vec![#(#config_key_entries),*]
            }
        }
    };

    Ok(quote! {
        impl #krate::beans::Bean for #name {
            type Deps = #deps_type;

            fn dependencies() -> Vec<(std::any::TypeId, &'static str)> {
                vec![#(#dep_type_ids),*]
            }

            #config_keys_fn

            const BUILD_VERSION: u64 = #build_version;

            fn build(ctx: &#krate::beans::BeanContext) -> Self {
                #config_prelude
                #live_config_prelude
                Self {
                    #(#field_inits,)*
                }
            }
        }

        impl #krate::beans::Registrable for #name {
            type Provided = Self;
            type Deps = #deps_type;

            fn register_into(registry: &mut #krate::beans::BeanRegistry) {
                registry.register::<Self>();
            }
        }
    })
}
