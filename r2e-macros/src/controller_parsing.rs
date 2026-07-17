use syn::parse::Parser;

use crate::type_utils::{parse_config_field, parse_config_section_prefix, unwrap_option_type};
use crate::types::*;

/// Parsed representation of a `#[controller(...)]` struct.
pub struct ControllerStructDef {
    pub name: syn::Ident,
    pub prefix: Option<String>,
    pub injected_fields: Vec<InjectedField>,
    pub identity_fields: Vec<IdentityField>,
    pub request_fields: Vec<RequestField>,
    pub config_fields: Vec<ConfigField>,
    pub config_section_fields: Vec<ConfigSectionField>,
}

impl ControllerStructDef {
    /// Names of every request-scoped field (identity + `#[inject(request)]`).
    /// These are removed from the physical controller core and live on the
    /// generated request façade instead.
    pub fn request_scoped_field_names(&self) -> Vec<syn::Ident> {
        self.identity_fields
            .iter()
            .map(|f| f.name.clone())
            .chain(self.request_fields.iter().map(|f| f.name.clone()))
            .collect()
    }

    /// (name, declared type) of every request-scoped field, in declaration
    /// order: the single optional identity field first, then request fields.
    /// Used to generate the request-data extractor and the façade fields.
    pub fn request_scoped_fields(&self) -> Vec<(&syn::Ident, &syn::Type)> {
        self.identity_fields
            .iter()
            .map(|f| (&f.name, &f.ty))
            .chain(self.request_fields.iter().map(|f| (&f.name, &f.ty)))
            .collect()
    }
}

/// Field helper attributes consumed by the `#[controller]` attribute macro.
///
/// These must be stripped from the emitted physical struct: once the derive is
/// gone they are no longer registered helper attributes, so leaving them on the
/// struct would produce "cannot find attribute" errors.
// `identity` is stripped only to keep the migration diagnostic targeted; it is
// no longer accepted as controller syntax.
pub const CONTROLLER_FIELD_ATTRS: &[&str] = &["inject", "identity", "config", "config_section"];

/// Check whether an `#[inject(...)]` attribute has the `identity` qualifier.
pub fn has_identity_qualifier(attr: &syn::Attribute) -> bool {
    inject_qualifier_is(attr, "identity")
}

/// Check whether an `#[inject(...)]` attribute has the `request` qualifier.
pub fn has_request_qualifier(attr: &syn::Attribute) -> bool {
    inject_qualifier_is(attr, "request")
}

fn inject_qualifier_is(attr: &syn::Attribute, want: &str) -> bool {
    if let syn::Meta::List(_) = &attr.meta {
        attr.parse_args::<syn::Ident>()
            .map(|ident| ident == want)
            .unwrap_or(false)
    } else {
        false
    }
}

/// Parse the `#[controller(path = "...")]` attribute arguments.
pub fn parse_controller_args(
    args: proc_macro2::TokenStream,
    span: proc_macro2::Span,
) -> syn::Result<Option<String>> {
    let mut prefix: Option<String> = None;

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("path") {
            let value = meta.value()?;
            let lit: syn::LitStr = value.parse()?;
            prefix = Some(lit.value());
            Ok(())
        } else if meta.path.is_ident("state") {
            Err(meta.error(
                "`state = ...` was removed — controllers are constructed from the bean graph \
                 by type; drop the key and make sure every #[inject] field type is provided or \
                 registered on the AppBuilder before build_state()",
            ))
        } else {
            Err(meta.error("unknown attribute in #[controller(...)]: expected `path`"))
        }
    });
    parser.parse2(args)?;
    let _ = span;

    Ok(prefix)
}

/// Parse a `#[controller]` struct into a [`ControllerStructDef`].
///
/// `prefix` comes from the attribute arguments; field scopes are read from
/// the struct's named fields.
pub fn parse(
    prefix: Option<String>,
    item: &syn::ItemStruct,
) -> syn::Result<ControllerStructDef> {
    let name = item.ident.clone();

    let fields: Vec<&syn::Field> = match &item.fields {
        syn::Fields::Named(named) => named.named.iter().collect(),
        syn::Fields::Unit => Vec::new(),
        syn::Fields::Unnamed(_) => {
            return Err(syn::Error::new(
                name.span(),
                "Controller cannot have tuple fields — use named fields or a unit struct:\n\
                 \n  struct MyController {\n      #[inject] service: MyService,\n  }\n\
                 \n  // or: struct MyController;",
            ))
        }
    };

    let mut injected_fields = Vec::new();
    let mut identity_fields = Vec::new();
    let mut request_fields = Vec::new();
    let mut config_fields = Vec::new();
    let mut config_section_fields = Vec::new();

    for field in fields {
        let field_name = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new(name.span(), "expected named field"))?;
        let field_type = field.ty.clone();

        let inject_attr = field.attrs.iter().find(|a| a.path().is_ident("inject"));
        let removed_identity_attr = field.attrs.iter().find(|a| a.path().is_ident("identity"));
        let config_attr = field.attrs.iter().find(|a| a.path().is_ident("config"));
        let config_section_attr = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("config_section"));

        if let Some(attr) = removed_identity_attr {
            return Err(syn::Error::new_spanned(
                attr,
                "`#[identity]` was removed; use `#[inject(identity)]`",
            ));
        } else if let Some(attr) = inject_attr {
            if has_identity_qualifier(attr) {
                // #[inject(identity)] -> request-scoped identity
                identity_fields.push(make_identity_field(field_name, field_type));
            } else if has_request_qualifier(attr) {
                // #[inject(request)] -> request-scoped extraction (any FromRequestParts)
                request_fields.push(RequestField {
                    name: field_name,
                    ty: field_type,
                });
            } else if matches!(attr.meta, syn::Meta::List(_)) {
                // #[inject(something_else)] -> error
                return Err(syn::Error::new_spanned(
                    attr,
                    "invalid qualifier in #[inject(...)]: only `identity` and `request` are supported\n\
                     \n  #[inject]           — app-scoped (cloned from state)\n\
                     \n  #[inject(identity)] — request-scoped identity extraction\n\
                     \n  #[inject(request)]  — request-scoped value via FromRequestParts",
                ));
            } else {
                // #[inject] -> app-scoped (clone from state)
                injected_fields.push(InjectedField {
                    name: field_name,
                    ty: field_type,
                });
            }
        } else if let Some(attr) = config_attr {
            let (key, ty_name) = parse_config_field(attr, &field_type)?;
            let is_option = crate::type_utils::is_option_type(&field_type);
            config_fields.push(ConfigField {
                name: field_name,
                key,
                ty_name,
                is_option,
            });
        } else if let Some(cs_attr) = config_section_attr {
            let prefix = parse_config_section_prefix(cs_attr)?;
            config_section_fields.push(ConfigSectionField {
                name: field_name,
                ty: field_type,
                prefix,
            });
        } else {
            return Err(syn::Error::new(
                field_name.span(),
                "every controller field must be annotated with one of:\n\
                 \n  #[inject]              — clone from app state\n\
                 \n  #[inject(identity)]    — extract from request (e.g. AuthenticatedUser)\n\
                 \n  #[inject(request)]     — extract from request via FromRequestParts\n\
                 \n  #[config(\"app.key\")]   — resolve from R2eConfig\n\
                 \n  #[config_section(prefix = \"...\")]  — resolve typed config section via ConfigProperties",
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
        prefix,
        injected_fields,
        identity_fields,
        request_fields,
        config_fields,
        config_section_fields,
    })
}

/// Build an [`IdentityField`], unwrapping `Option<T>` so guards see `Option<&T>`.
fn make_identity_field(name: syn::Ident, declared: syn::Type) -> IdentityField {
    let (inner_ty, is_optional) = match unwrap_option_type(&declared) {
        Some(inner) => (inner.clone(), true),
        None => (declared.clone(), false),
    };
    IdentityField {
        name,
        ty: declared,
        inner_ty,
        is_optional,
    }
}
