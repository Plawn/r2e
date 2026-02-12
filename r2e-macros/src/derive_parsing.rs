use crate::types::*;

/// Parsed representation of a `#[derive(Controller)]` struct.
pub struct ControllerStructDef {
    pub name: syn::Ident,
    pub state_type: syn::Path,
    pub prefix: Option<String>,
    pub injected_fields: Vec<InjectedField>,
    pub identity_fields: Vec<IdentityField>,
    pub config_fields: Vec<ConfigField>,
    pub config_section_fields: Vec<ConfigSectionField>,
    pub is_unit_struct: bool,
}

/// Check whether an `#[inject(...)]` attribute has the `identity` qualifier.
pub fn has_identity_qualifier(attr: &syn::Attribute) -> bool {
    if let syn::Meta::List(_) = &attr.meta {
        attr.parse_args::<syn::Ident>()
            .map(|ident| ident == "identity")
            .unwrap_or(false)
    } else {
        false
    }
}

pub fn parse(input: syn::DeriveInput) -> syn::Result<ControllerStructDef> {
    let name = input.ident;

    // Parse #[controller(path = "...", state = ...)]
    let mut state_type: Option<syn::Path> = None;
    let mut prefix: Option<String> = None;

    for attr in &input.attrs {
        if attr.path().is_ident("controller") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("path") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    prefix = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("state") {
                    let value = meta.value()?;
                    state_type = Some(value.parse()?);
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown attribute in #[controller(...)]: expected `path` or `state`"
                    ))
                }
            })?;
        }
    }

    let state_type = state_type.ok_or_else(|| {
        syn::Error::new(
            name.span(),
            "#[controller(state = ...)] is required\n\
             example: #[controller(path = \"/users\", state = AppState)]",
        )
    })?;

    // Parse fields
    let (fields, is_unit_struct) = match input.data {
        syn::Data::Struct(data) => match data.fields {
            syn::Fields::Named(named) => (named.named, false),
            syn::Fields::Unit => (syn::punctuated::Punctuated::new(), true),
            syn::Fields::Unnamed(_) => {
                return Err(syn::Error::new(
                    name.span(),
                    "Controller cannot have tuple fields — use named fields or a unit struct:\n\
                     \n  struct MyController {\n      #[inject] service: MyService,\n  }\n\
                     \n  // or: struct MyController;",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new(
                name.span(),
                "#[derive(Controller)] only works on structs — enums and unions are not supported",
            ))
        }
    };

    let mut injected_fields = Vec::new();
    let mut identity_fields = Vec::new();
    let mut config_fields = Vec::new();
    let mut config_section_fields = Vec::new();

    for field in fields {
        let field_name = field.ident.clone().ok_or_else(|| {
            syn::Error::new(name.span(), "expected named field")
        })?;
        let field_type = field.ty.clone();

        let inject_attr = field.attrs.iter().find(|a| a.path().is_ident("inject"));
        let legacy_identity = field.attrs.iter().any(|a| a.path().is_ident("identity"));
        let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));
        let config_section_attr = field.attrs.iter().any(|a| a.path().is_ident("config_section"));

        if let Some(attr) = inject_attr {
            if has_identity_qualifier(attr) {
                // #[inject(identity)] -> request-scoped identity
                identity_fields.push(IdentityField {
                    name: field_name,
                    ty: field_type,
                });
            } else if matches!(attr.meta, syn::Meta::List(_)) {
                // #[inject(something_else)] -> error
                return Err(syn::Error::new_spanned(
                    attr,
                    "invalid qualifier in #[inject(...)]: only `identity` is supported\n\
                     \n  #[inject]           — app-scoped (cloned from state)\n\
                     \n  #[inject(identity)] — request-scoped identity extraction",
                ));
            } else {
                // #[inject] -> app-scoped (clone from state)
                injected_fields.push(InjectedField {
                    name: field_name,
                    ty: field_type,
                });
            }
        } else if legacy_identity {
            // backward compat: #[identity] -> identity field
            identity_fields.push(IdentityField {
                name: field_name,
                ty: field_type,
            });
        } else if let Some(attr) = config_attr {
            let key: syn::LitStr = attr.parse_args()?;
            config_fields.push(ConfigField {
                name: field_name,
                ty: field_type,
                key: key.value(),
            });
        } else if config_section_attr {
            config_section_fields.push(ConfigSectionField {
                name: field_name,
                ty: field_type,
            });
        } else {
            return Err(syn::Error::new(
                field_name.span(),
                "every controller field must be annotated with one of:\n\
                 \n  #[inject]              — clone from app state\n\
                 \n  #[inject(identity)]    — extract from request (e.g. AuthenticatedUser)\n\
                 \n  #[config(\"app.key\")]   — resolve from R2eConfig\n\
                 \n  #[config_section]      — resolve typed config section via ConfigProperties",
            ));
        }
    }

    if identity_fields.len() > 1 {
        return Err(syn::Error::new(
            name.span(),
            "controller can have at most one #[inject(identity)] struct field\n\n\
             hint: use param-level injection for mixed public/protected endpoints:\n\
             \n  #[get(\"/me\")]\n  async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> ... { }",
        ));
    }

    Ok(ControllerStructDef {
        name,
        state_type,
        prefix,
        injected_fields,
        identity_fields,
        config_fields,
        config_section_fields,
        is_unit_struct,
    })
}
