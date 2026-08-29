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

/// Unwrap `Result<T, E>` / `ApiResult<T>` / `JsonResult<T>` to `T`, leaving
/// every other type unchanged.
pub fn unwrap_result_type(ty: &Type) -> &Type {
    result_ok_type(ty).unwrap_or(ty)
}

/// Extract `T` from `Json<T>` (including fully-qualified paths).
pub fn unwrap_json_type(ty: &Type) -> Option<&Type> {
    if !type_last_segment_is(ty, "Json") {
        return None;
    }
    let Type::Path(type_path) = ty else {
        return None;
    };
    let syn::PathArguments::AngleBracketed(args) = &type_path.path.segments.last()?.arguments
    else {
        return None;
    };
    match args.args.first() {
        Some(syn::GenericArgument::Type(inner)) => Some(inner),
        _ => None,
    }
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

/// The single diagnostic every host emits for a `name = "..."` bean qualifier.
///
/// R2E has no named beans: the graph is keyed by type, and the only way to have
/// two beans of the same underlying type is to give them two types. Emitted
/// verbatim by `#[inject(name = ...)]` (every host) and by
/// `#[producer(name = ...)]`, so the fix reads identically everywhere.
pub const NAMED_BEAN_MSG: &str =
    "named beans are not supported: R2E has no bean qualifiers — the bean graph is keyed by type\n\
     \n  declare a newtype instead, and inject it by type:\
     \n\
     \n    #[derive(Clone)]\
     \n    pub struct ReadPool(pub PgPool);\
     \n\
     \n    #[producer] fn read_pool(..) -> ReadPool { .. }\
     \n    #[inject] pool: ReadPool,";

/// True when an `#[inject(...)]` attribute carries a `name = "..."` argument.
///
/// Parse failures answer `false`: the host's own argument validation reports
/// them, and a malformed attribute is not a named-bean declaration.
fn inject_has_name(attr: &syn::Attribute) -> bool {
    let syn::Meta::List(list) = &attr.meta else {
        return false;
    };
    let mut found = false;
    let _ = list.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            found = true;
        }
        // Consume `= <value>` so parsing reaches every argument.
        if meta.input.peek(syn::Token![=]) {
            let _: syn::Expr = meta.value()?.parse()?;
        }
        Ok(())
    });
    found
}

/// Reject `#[inject(name = "...")]` on any host — fields or parameters.
///
/// The single shared rejection: controllers, `#[bean]` / `#[producer]` params,
/// `#[derive(Bean)]`, `#[derive(DecoratorBean)]` and
/// `#[derive(BackgroundService)]` fields all call it, so the message is the same
/// wherever a user reaches for a qualifier.
pub fn reject_named_inject(attrs: &[syn::Attribute]) -> syn::Result<()> {
    for attr in attrs {
        if attr.path().is_ident("inject") && inject_has_name(attr) {
            return Err(syn::Error::new_spanned(attr, NAMED_BEAN_MSG));
        }
    }
    Ok(())
}

/// Validate `#[inject(...)]` arguments on a **bean-like** host: `#[bean]` and
/// `#[producer]` parameters, `#[derive(Bean)]` / `#[derive(DecoratorBean)]` /
/// `#[derive(BackgroundService)]` fields.
///
/// These hosts are app-scoped only, so `#[inject]` takes no arguments at all —
/// `identity` / `request` are `#[controller]`-only request scopes and
/// `name = "..."` does not exist anywhere.
pub fn check_bean_inject_args(attrs: &[syn::Attribute]) -> syn::Result<()> {
    reject_named_inject(attrs)?;
    for attr in attrs {
        if !attr.path().is_ident("inject") {
            continue;
        }
        if matches!(attr.meta, syn::Meta::List(_)) {
            return Err(syn::Error::new_spanned(
                attr,
                "`#[inject(...)]` takes no arguments here: this host is app-scoped\n\
                 \n  #[inject]  — clone an app-scoped bean\
                 \n\
                 \n`identity` / `request` are request scopes, available on `#[controller]` fields only.",
            ));
        }
    }
    Ok(())
}

/// Parse a `#[config("app.key")]` attribute against its declared type, producing
/// the key and a stringified type name.
pub fn parse_config_field(attr: &syn::Attribute, ty: &Type) -> syn::Result<(String, String)> {
    let key: syn::LitStr = attr.parse_args()?;
    let key = key.value();
    let ty_name = quote!(#ty).to_string();
    Ok((key, ty_name))
}

/// Parse a `#[live_config("app.key")]` attribute against its declared type,
/// producing the key and a stringified type name.
///
/// Same shape as [`parse_config_field`], but a bare `#[live_config]` gets a
/// targeted error instead of syn's generic "expected attribute arguments"
/// message.
pub fn parse_live_config_field(attr: &syn::Attribute, ty: &Type) -> syn::Result<(String, String)> {
    if !matches!(attr.meta, syn::Meta::List(_)) {
        return Err(syn::Error::new_spanned(
            attr,
            "#[live_config] requires a config key:\n\
             \n  #[live_config(\"app.key\")] url: LiveConfig<String>",
        ));
    }
    parse_config_field(attr, ty)
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

/// Split a literal `Result<T, E>` into `(T, E)`.
///
/// Only a path whose last segment is `Result` with exactly two angle-bracketed
/// type arguments matches. Single-argument aliases (`ApiResult<T>`,
/// `JsonResult<T>`) deliberately do NOT: bean and producer constructors need
/// both halves (`Self::Error` is the second), and an alias hides one of them.
pub fn result_ok_err_types(ty: &Type) -> Option<(&Type, &Type)> {
    let Type::Path(p) = ty else { return None };
    let last = p.path.segments.last()?;
    if last.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    let tys: Vec<&Type> = args
        .args
        .iter()
        .filter_map(|a| match a {
            syn::GenericArgument::Type(t) => Some(t),
            _ => None,
        })
        .collect();
    if tys.len() == 2 && args.args.len() == 2 {
        Some((tys[0], tys[1]))
    } else {
        None
    }
}
