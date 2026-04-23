use quote::quote;
use syn::Type;

/// Check if a type is `Option<T>`.
pub fn is_option_type(ty: &Type) -> bool {
    unwrap_option_type(ty).is_some()
}

/// Check whether `ty` is a path type whose last segment ident equals `name`.
///
/// Matches `Name`, `a::b::Name`, `Name<T>`, etc. — but not `NotName`, `NameExt`,
/// or types with a different last segment. Non-path types always return `false`.
pub fn type_last_segment_is(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == name))
}

/// Check whether `ty` is a `Result<_, _>`-shaped type, matching any of the
/// framework's Result aliases (`Result`, `ApiResult`, `JsonResult`).
pub fn is_result_like(ty: &Type) -> bool {
    let Type::Path(p) = ty else { return false };
    let Some(last) = p.path.segments.last() else { return false };
    matches!(
        last.ident.to_string().as_str(),
        "Result" | "ApiResult" | "JsonResult"
    )
}

/// If `ty` is `Option<X>` (or `std::option::Option<X>`), return `Some(X)`.
/// Otherwise, return `None`.
pub fn unwrap_option_type(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else { return None };
    let segments = &type_path.path.segments;

    // Match `Option<X>` or `std::option::Option<X>`
    let last = segments.last()?;
    if last.ident != "Option" {
        return None;
    }

    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };

    if args.args.len() != 1 {
        return None;
    }

    match &args.args[0] {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

/// Convert a snake_case name to PascalCase.
pub fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// Extract the base name of a type (e.g., `SqlitePool` from `sqlx::SqlitePool`).
pub fn type_base_name(ty: &Type) -> String {
    match ty {
        Type::Path(type_path) => {
            if let Some(last) = type_path.path.segments.last() {
                last.ident.to_string()
            } else {
                quote!(#ty).to_string()
            }
        }
        _ => quote!(#ty).to_string(),
    }
}

/// Build the newtype identifier for a named bean: `PascalName` + type base name.
///
/// E.g., `name = "primary"` on `SqlitePool` → `PrimarySqlitePool`.
pub fn named_bean_newtype_ident(name: &str, ty: &Type) -> syn::Ident {
    let pascal_name = to_pascal_case(name);
    let base = type_base_name(ty);
    syn::Ident::new(&format!("{}{}", pascal_name, base), proc_macro2::Span::call_site())
}

/// Parse `#[inject(name = "...")]` from attributes, returning the name if present.
///
/// Returns `Ok(None)` if no `#[inject(name = "...")]` is found.
/// Returns `Ok(Some(name))` if found. Bare `#[inject]` is accepted.
/// Any other argument (including `identity`, unknown keys) is rejected — for
/// controller fields the identity qualifier is parsed by `has_identity_qualifier`
/// before `parse_inject_name` is called; beans/producers have no identity.
pub fn parse_inject_name(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if attr.path().is_ident("inject") {
            if let syn::Meta::List(_) = &attr.meta {
                // `#[inject(identity)]` is the controller-only bare-ident form
                // and is consumed by `has_identity_qualifier` before this
                // function runs. In bean/producer contexts we must still let
                // the controller-parsing path reach it, so accept it silently
                // here and let the caller decide whether it is valid.
                if let Ok(ident) = attr.parse_args::<syn::Ident>() {
                    if ident == "identity" {
                        continue;
                    }
                }

                let mut name = None;
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("name") {
                        let value = meta.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        name = Some(lit.value());
                        Ok(())
                    } else {
                        Err(meta.error(
                            "unknown `#[inject]` argument; expected `identity` or `name = \"...\"`",
                        ))
                    }
                })?;
                if let Some(n) = name {
                    return Ok(Some(n));
                }
            }
        }
    }
    Ok(None)
}

/// Parse a `#[config("app.key")]` attribute against its declared type, producing
/// the key, the matching `APP_KEY` env-var hint, and a stringified type name.
pub fn parse_config_field(
    attr: &syn::Attribute,
    ty: &Type,
) -> syn::Result<(String, String, String)> {
    let key: syn::LitStr = attr.parse_args()?;
    let key = key.value();
    let env_hint = key.replace('.', "_").to_uppercase();
    let ty_name = quote!(#ty).to_string();
    Ok((key, env_hint, ty_name))
}

/// Parse `#[config_section(prefix = "...")]` and return the prefix string.
pub fn parse_config_section_prefix(attr: &syn::Attribute) -> syn::Result<String> {
    let mut prefix: Option<String> = None;
    if let syn::Meta::List(_) = &attr.meta {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("prefix") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                prefix = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error("expected `prefix` in #[config_section(prefix = \"...\")]"))
            }
        })?;
    }
    prefix.ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "#[config_section] requires a prefix: #[config_section(prefix = \"app\")]",
        )
    })
}
