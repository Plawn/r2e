use crate::types::*;

/// Parsed representation of a `#[derive(Controller)]` struct.
pub struct ControllerStructDef {
    pub name: syn::Ident,
    pub state_type: syn::Path,
    pub prefix: Option<String>,
    pub injected_fields: Vec<InjectedField>,
    pub identity_fields: Vec<IdentityField>,
    pub config_fields: Vec<ConfigField>,
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
                    Err(meta.error("expected `path` or `state`"))
                }
            })?;
        }
    }

    let state_type = state_type.ok_or_else(|| {
        syn::Error::new(name.span(), "#[controller(state = ...)] is required")
    })?;

    // Parse fields
    let fields = match input.data {
        syn::Data::Struct(data) => match data.fields {
            syn::Fields::Named(named) => named.named,
            _ => {
                return Err(syn::Error::new(
                    name.span(),
                    "Controller must have named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new(
                name.span(),
                "#[derive(Controller)] only works on structs",
            ))
        }
    };

    let mut injected_fields = Vec::new();
    let mut identity_fields = Vec::new();
    let mut config_fields = Vec::new();

    for field in fields {
        let field_name = field.ident.clone().ok_or_else(|| {
            syn::Error::new(name.span(), "expected named field")
        })?;
        let field_type = field.ty.clone();

        let is_inject = field.attrs.iter().any(|a| a.path().is_ident("inject"));
        let is_identity = field.attrs.iter().any(|a| a.path().is_ident("identity"));
        let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));

        if is_inject {
            injected_fields.push(InjectedField {
                name: field_name,
                ty: field_type,
            });
        } else if is_identity {
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
        } else {
            return Err(syn::Error::new(
                field_name.span(),
                "field in controller must have #[inject], #[identity], or #[config(\"key\")]",
            ));
        }
    }

    Ok(ControllerStructDef {
        name,
        state_type,
        prefix,
        injected_fields,
        identity_fields,
        config_fields,
    })
}
