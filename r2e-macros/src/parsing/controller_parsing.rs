use syn::parse::Parser;

use crate::model::types::*;
use crate::util::type_utils::{
    parse_config_field, parse_config_section_prefix, parse_live_config_field, unwrap_option_type,
};

/// Parsed representation of a `#[controller(...)]` struct.
pub struct ControllerStructDef {
    pub name: syn::Ident,
    pub prefix: Option<String>,
    /// `#[controller(tag = "…")]` — the OpenAPI tag this controller's
    /// operations are grouped under. `None` means "use the struct name", the
    /// historical behavior.
    pub tag: Option<String>,
    pub injected_fields: Vec<InjectedField>,
    pub identity_fields: Vec<IdentityField>,
    pub request_fields: Vec<RequestField>,
    pub config_fields: Vec<ConfigField>,
    pub live_config_fields: Vec<LiveConfigField>,
    pub config_section_fields: Vec<ConfigSectionField>,
    /// Errors that must be reported WITHOUT aborting codegen.
    ///
    /// A hard `Err` from the parser leaves `#[controller]` emitting only the
    /// struct, so `#[routes]` then fails a second time on the missing
    /// `ContextConstruct` / `EndpointDeps` impls and the real diagnostic is
    /// buried under two unrelated trait errors. Anything the macro can recover
    /// from (an unhonourable attribute it simply drops) lands here instead and
    /// is emitted as a `compile_error!` alongside the full, normal expansion.
    pub deferred_errors: Vec<syn::Error>,
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
    pub fn request_scoped_fields(&self) -> Vec<RequestScopedField<'_>> {
        self.identity_fields
            .iter()
            .map(|f| RequestScopedField {
                name: &f.name,
                ty: &f.ty,
                attrs: &f.attrs,
            })
            .chain(self.request_fields.iter().map(|f| RequestScopedField {
                name: &f.name,
                ty: &f.ty,
                attrs: &f.attrs,
            }))
            .collect()
    }
}

/// One request-scoped field (identity or `#[inject(request)]`), as the codegen
/// needs it: the declaration re-emitted on the request-data extractor and the
/// façade keeps the user's own attributes.
pub struct RequestScopedField<'a> {
    pub name: &'a syn::Ident,
    pub ty: &'a syn::Type,
    pub attrs: &'a [syn::Attribute],
}

/// Field helper attributes consumed by the `#[controller]` attribute macro.
///
/// These must be stripped from the emitted physical struct: once the derive is
/// gone they are no longer registered helper attributes, so leaving them on the
/// struct would produce "cannot find attribute" errors.
// `identity` is stripped only to keep the migration diagnostic targeted; it is
// no longer accepted as controller syntax.
pub const CONTROLLER_FIELD_ATTRS: &[&str] = &[
    "inject",
    "identity",
    "config",
    "live_config",
    "config_section",
];

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

/// The `#[controller(...)]` attribute arguments.
pub struct ControllerArgs {
    pub prefix: Option<String>,
    pub tag: Option<String>,
}

/// Parse the `#[controller(path = "...", tag = "...")]` attribute arguments.
pub fn parse_controller_args(
    args: proc_macro2::TokenStream,
    span: proc_macro2::Span,
) -> syn::Result<ControllerArgs> {
    let mut prefix: Option<String> = None;
    let mut tag: Option<String> = None;

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("path") {
            let value = meta.value()?;
            let lit: syn::LitStr = value.parse()?;
            prefix = Some(lit.value());
            Ok(())
        } else if meta.path.is_ident("tag") {
            let value = meta.value()?;
            let lit: syn::LitStr = value.parse()?;
            let value = lit.value();
            if value.trim().is_empty() {
                return Err(syn::Error::new_spanned(
                    &lit,
                    "`tag` must not be empty — drop the key to keep the struct name as the \
                     OpenAPI tag",
                ));
            }
            tag = Some(value);
            Ok(())
        } else if meta.path.is_ident("state") {
            Err(meta.error(
                "`state = ...` was removed — controllers are constructed from the bean graph \
                 by type; drop the key and make sure every #[inject] field type is provided or \
                 registered on the AppBuilder before build_state()",
            ))
        } else {
            Err(meta.error("unknown attribute in #[controller(...)]: expected `path` or `tag`"))
        }
    });
    parser.parse2(args)?;
    let _ = span;

    Ok(ControllerArgs { prefix, tag })
}

/// Parse a `#[controller]` struct into a [`ControllerStructDef`].
///
/// `args` comes from the attribute arguments; field scopes are read from
/// the struct's named fields.
pub fn parse(args: ControllerArgs, item: &syn::ItemStruct) -> syn::Result<ControllerStructDef> {
    let ControllerArgs { prefix, tag } = args;
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
    let mut live_config_fields = Vec::new();
    let mut config_section_fields = Vec::new();
    let mut deferred_errors: Vec<syn::Error> = Vec::new();

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
        let live_config_attr = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("live_config"));

        // `#[live_config]` is app-scoped: stacking it on a request-scoped
        // `#[inject(identity)]` / `#[inject(request)]` field mixes two scopes on
        // one slot. Checked before the shared exclusivity rule so the message
        // names the request scope.
        if let (Some(live), Some(inject)) = (live_config_attr, inject_attr) {
            if has_identity_qualifier(inject) || has_request_qualifier(inject) {
                return Err(syn::Error::new_spanned(
                    live,
                    "#[live_config] is app-scoped and cannot be combined with a request-scoped \
                     #[inject(identity)] / #[inject(request)] field\n\
                     \n  hint: declare the LiveConfig<T> handle on its own field",
                ));
            }
        }
        crate::model::field_resolver::check_live_config_exclusive(&field.attrs)?;

        if let Some(attr) = live_config_attr {
            let (key, ty_name) = parse_live_config_field(attr, &field_type)?;
            live_config_fields.push(LiveConfigField {
                name: field_name,
                ty: field_type,
                key,
                ty_name,
            });
        } else if let Some(attr) = removed_identity_attr {
            return Err(syn::Error::new_spanned(
                attr,
                "`#[identity]` was removed; use `#[inject(identity)]`",
            ));
        } else if let Some(attr) = inject_attr {
            if has_identity_qualifier(attr) {
                // #[inject(identity)] -> request-scoped identity
                deferred_errors.extend(cfg_on_request_scoped_field(&field.attrs));
                identity_fields.push(make_identity_field(
                    field_name,
                    field_type,
                    user_field_attrs(&field.attrs),
                ));
            } else if has_request_qualifier(attr) {
                // #[inject(request)] -> request-scoped extraction (any FromRequestParts)
                deferred_errors.extend(cfg_on_request_scoped_field(&field.attrs));
                request_fields.push(RequestField {
                    name: field_name,
                    ty: field_type,
                    attrs: user_field_attrs(&field.attrs),
                });
            } else if matches!(attr.meta, syn::Meta::List(_)) {
                // `#[inject(name = "...")]` gets the shared named-bean rejection
                // so the fix reads identically on every host.
                crate::util::type_utils::reject_named_inject(std::slice::from_ref(attr))?;
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
            let is_option = crate::util::type_utils::is_option_type(&field_type);
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
                 \n  #[live_config(\"app.key\")]  — runtime-updatable LiveConfig<T> handle\n\
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
        tag,
        injected_fields,
        identity_fields,
        request_fields,
        config_fields,
        live_config_fields,
        config_section_fields,
        deferred_errors,
    })
}

/// The attributes a request-scoped field must carry over to the re-declared
/// field: everything except the `#[controller]` helper attributes, which stop
/// being registered helpers once the macro is gone.
fn user_field_attrs(attrs: &[syn::Attribute]) -> Vec<syn::Attribute> {
    attrs
        .iter()
        .filter(|a| {
            !CONTROLLER_FIELD_ATTRS
                .iter()
                .any(|name| a.path().is_ident(name))
                // Recorded as a deferred error by `cfg_on_request_scoped_field`
                // and dropped here: re-emitting it would gate the façade field
                // while the positional marker tuple stays ungated, burying the
                // real diagnostic under a type mismatch.
                && !a.path().is_ident("cfg")
                && !a.path().is_ident("cfg_attr")
        })
        .cloned()
        .collect()
}

/// `#[cfg]` / `#[cfg_attr]` on a request-scoped field cannot be honoured, so it
/// is reported rather than silently ignored.
///
/// Unlike item-level `#[cfg]`, a FIELD-level one is NOT pre-evaluated by rustc:
/// it arrives here verbatim. Honouring it would mean cfg-gating the request-data
/// struct field, its extraction, its entry in the positional marker tuple, its
/// `FromRequestPartsVia` bound and the façade binding — and a tuple type cannot
/// be cfg-gated element-wise. A silent no-op on a *conditional* field is a
/// security-shaped surprise (the extractor keeps running), so this is a hard
/// error (task #985).
///
/// Returned rather than raised: the caller records it as a deferred error and
/// the field is kept (minus the offending attribute) so the rest of the
/// expansion still happens and `#[routes]` has nothing extra to complain about.
fn cfg_on_request_scoped_field(attrs: &[syn::Attribute]) -> Option<syn::Error> {
    let attr = attrs
        .iter()
        .find(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr"))?;
    let name = if attr.path().is_ident("cfg") {
        "#[cfg]"
    } else {
        "#[cfg_attr]"
    };
    Some(syn::Error::new_spanned(
        attr,
        format!(
            "{name} is not supported on a request-scoped controller field \
             (#[inject(identity)] / #[inject(request)])\n\n\
             The field is re-declared on the generated request extractor and \
             the façade, whose marker tuple is positional — the macro cannot \
             gate one element of it.\n\n\
             \x20 hint: cfg the whole controller instead, or make the field \
             type itself conditional via a type alias"
        ),
    ))
}

/// Build an [`IdentityField`], unwrapping `Option<T>` so guards see `Option<&T>`.
fn make_identity_field(
    name: syn::Ident,
    declared: syn::Type,
    attrs: Vec<syn::Attribute>,
) -> IdentityField {
    let (inner_ty, is_optional) = match unwrap_option_type(&declared) {
        Some(inner) => (inner.clone(), true),
        None => (declared.clone(), false),
    };
    IdentityField {
        name,
        attrs,
        ty: declared,
        inner_ty,
        is_optional,
    }
}
