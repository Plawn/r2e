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
    let Some(last) = p.path.segments.last() else {
        return false;
    };
    matches!(
        last.ident.to_string().as_str(),
        "Result" | "ApiResult" | "JsonResult"
    )
}

/// Return `true` if `ty` is the unit type `()`.
pub fn is_unit_type(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(t) if t.elems.is_empty())
}

/// If `ty` is a `Result`-shaped type (`Result`/`ApiResult`/`JsonResult`),
/// return its first (`Ok`) type argument. Returns `None` for non-`Result`
/// types or aliases with no angle-bracketed arguments.
pub fn result_ok_type(ty: &Type) -> Option<&Type> {
    if !is_result_like(ty) {
        return None;
    }
    let Type::Path(p) = ty else { return None };
    let last = p.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    args.args.iter().find_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

/// If `ty` is `Option<X>` (or `std::option::Option<X>`), return `Some(X)`.
/// Otherwise, return `None`.
pub fn unwrap_option_type(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else {
        return None;
    };
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
    syn::Ident::new(
        &format!("{}{}", pascal_name, base),
        proc_macro2::Span::call_site(),
    )
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
/// the key and a stringified type name.
pub fn parse_config_field(attr: &syn::Attribute, ty: &Type) -> syn::Result<(String, String)> {
    let key: syn::LitStr = attr.parse_args()?;
    let key = key.value();
    let ty_name = quote!(#ty).to_string();
    Ok((key, ty_name))
}

/// Build the actionable remediation sentence appended to a required-config
/// panic message.
///
/// The `R2E_` overlay mapping is strict (`_`→`.`, nothing else), so a key
/// containing `-` or an in-segment `_` (`database.min-idle`,
/// `database.max_idle`) is **not** addressable via any `R2E_` var — those
/// keys point at YAML / `${VAR}` placeholders. Purely dotted keys name their
/// full working var, `R2E_` prefix included (unprefixed env vars are ignored
/// by the overlay).
pub fn config_hint_sentence(key: &str) -> String {
    if key.contains('-') || key.contains('_') {
        "Add it to application.yaml (keys containing '-' or '_' are not addressable via R2E_ env vars; use a ${VAR} placeholder for env-driven values).".to_string()
    } else {
        let env = key.replace('.', "_").to_uppercase();
        format!("Add it to application.yaml or set env var `R2E_{env}`.")
    }
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

/// Rewrite every `'_` (inferred) lifetime in a type to `'static`, so the type
/// can appear in a where-clause bound (E0637 forbids `'_` there).
///
/// Used for `#[managed]` parameter types like `&mut Tx<'_, Sqlite>`: the
/// handler's expression position infers the lifetime, but the generated
/// `Ty: ManagedResource<S>` bound must name it — and `ManagedResource`
/// resources are `'static` by construction.
pub fn staticize_lifetimes(ty: &Type) -> Type {
    use syn::visit_mut::VisitMut;
    struct Staticize;
    impl VisitMut for Staticize {
        fn visit_lifetime_mut(&mut self, lt: &mut syn::Lifetime) {
            if lt.ident == "_" {
                lt.ident = syn::Ident::new("static", lt.ident.span());
            }
        }
    }
    let mut ty = ty.clone();
    Staticize.visit_type_mut(&mut ty);
    ty
}
