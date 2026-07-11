use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Lit, Meta};

use crate::crate_path::r2e_core_path;

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match generate(&input) {
        Ok(output) => output.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Parsed information about a single field in a ConfigProperties struct.
struct FieldInfo {
    name: syn::Ident,
    ty: syn::Type,
    /// The config default value expression, if any.
    default_expr: Option<TokenStream2>,
    /// The config default value as string for metadata.
    default_str: Option<String>,
    /// Custom config key override from `#[config(key = "...")]`.
    custom_key: Option<String>,
    /// Whether the field is Option<T>.
    is_option: bool,
    /// Doc comment text.
    doc: Option<String>,
    /// Whether this is a `#[config(section)]` — nested ConfigProperties.
    is_section: bool,
    /// Whether this section is map-valued (`HashMap`/`BTreeMap<String, T>`).
    is_map_section: bool,
    /// Whether the field has a bare `#[config(default)]` (Default::default()
    /// fallback for absent sections).
    default_flag: bool,
    /// Whether the field is `#[config(skip)]` — not read from config.
    is_skip: bool,
    /// Explicit env var from `#[config(env = "...")]`.
    env_var: Option<String>,
    /// Whether the field has any `#[validate(...)]` attributes (from garde).
    has_validate: bool,
}

use crate::type_utils::is_option_type;
use crate::type_utils::unwrap_option_type as option_inner_type;

/// Extract doc comments from a field's attributes.
fn extract_doc_comment(attrs: &[syn::Attribute]) -> Option<String> {
    let docs: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if attr.path().is_ident("doc") {
                if let Meta::NameValue(nv) = &attr.meta {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: Lit::Str(s), ..
                    }) = &nv.value
                    {
                        return Some(s.value().trim().to_string());
                    }
                }
            }
            None
        })
        .collect();
    if docs.is_empty() {
        None
    } else {
        Some(docs.join(" "))
    }
}

/// Parsed field-level `#[config(...)]` attributes.
struct FieldConfig {
    default: Option<(TokenStream2, String)>,
    /// True if the default expression is a string literal (needs `.into()` for String conversion).
    default_is_str_lit: bool,
    /// Bare `default` (no `= value`) — Default::default() fallback for absent sections.
    default_flag: bool,
    key: Option<String>,
    section: bool,
    env: Option<String>,
    skip: bool,
}

/// Extract `#[config(default = <expr>, key = "...", section, env = "...", skip)]` from a field's attributes.
fn extract_field_config(attrs: &[syn::Attribute]) -> syn::Result<FieldConfig> {
    let mut result = FieldConfig {
        default: None,
        default_is_str_lit: false,
        default_flag: false,
        key: None,
        section: false,
        env: None,
        skip: false,
    };
    for attr in attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("default") {
                    if !meta.input.peek(syn::Token![=]) {
                        result.default_flag = true;
                        return Ok(());
                    }
                    let value = meta.value()?;
                    let lit: syn::Expr = value.parse()?;
                    // String literals need .into() for &str → String, and their
                    // metadata string is the unquoted value.
                    let lit_str = if let syn::Expr::Lit(syn::ExprLit { lit: Lit::Str(s), .. }) = &lit {
                        result.default_is_str_lit = true;
                        s.value()
                    } else {
                        quote!(#lit).to_string()
                    };
                    result.default = Some((quote!(#lit), lit_str));
                    Ok(())
                } else if meta.path.is_ident("key") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    result.key = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("section") {
                    result.section = true;
                    Ok(())
                } else if meta.path.is_ident("env") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    result.env = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("skip") {
                    result.skip = true;
                    Ok(())
                } else {
                    Err(meta.error("expected `default`, `key`, `section`, `env`, or `skip` in #[config(...)]"))
                }
            })?;
        }
    }
    Ok(result)
}

/// Detect `HashMap<String, V>` / `BTreeMap<String, V>` and return `V`.
fn string_map_value_type(ty: &syn::Type) -> Option<&syn::Type> {
    let syn::Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != "HashMap" && seg.ident != "BTreeMap" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    if args.args.len() != 2 {
        return None;
    }
    let syn::GenericArgument::Type(key_ty) = args.args.first()? else {
        return None;
    };
    let syn::GenericArgument::Type(value_ty) = args.args.last()? else {
        return None;
    };
    let is_string_key = matches!(
        key_ty,
        syn::Type::Path(kp) if kp.path.segments.last().map(|s| s.ident == "String").unwrap_or(false)
    );
    if !is_string_key {
        return None;
    }
    Some(value_ty)
}

/// Check if a field has any `#[validate(...)]` attributes (from garde).
fn has_validate_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("validate"))
}

/// Consume and ignore a `#[config(prefix = "...")]` attribute if present (for backwards compat).
/// Returns Ok(()) whether or not the attribute is present.
fn consume_prefix_attr(input: &DeriveInput) -> syn::Result<()> {
    for attr in &input.attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("prefix") {
                    let value = meta.value()?;
                    let _lit: syn::LitStr = value.parse()?;
                    Ok(())
                } else {
                    Err(meta.error("expected `prefix` in #[config(prefix = \"...\")]"))
                }
            })?;
        }
    }
    Ok(())
}

/// Presence probe stored in `PropertyMeta::resolvable` — the validation-side
/// twin of the `from_config` resolution codegen (`field_inits` below). It must
/// consult the same explicit-value sources in the same order (config map →
/// custom `#[config(env = "...")]` var), minus defaults. Any new fallback
/// source added to the `from_config` codegen (e.g. a secrets provider) must
/// get a matching probe here — `validate_section` has no other knowledge of
/// resolution semantics.
fn resolvability_probe(f: &FieldInfo, krate: &TokenStream2) -> TokenStream2 {
    if f.is_section {
        // A section resolves when any key lives under its prefix — mirrors
        // the `has_prefix` presence check in the section `field_inits`.
        // Unreachable through `validate_section` (sections are never
        // `required`); it serves the public `is_resolvable` oracle.
        return quote! {
            |__config, __meta| __config.has_prefix(&__meta.full_key)
        };
    }
    // Config map → custom env var, both read as data from the meta itself
    // (`full_key` / `env_var`), so the probe cannot desync from the fields.
    quote! { #krate::config::typed::PropertyMeta::standard_sources }
}

fn generate(input: &DeriveInput) -> syn::Result<TokenStream2> {
    match &input.data {
        Data::Struct(data) => generate_struct(input, data),
        Data::Enum(data) => generate_enum(input, data),
        _ => Err(syn::Error::new_spanned(
            &input.ident,
            "#[derive(ConfigProperties)] only works on structs with named fields and tagged enums",
        )),
    }
}

fn generate_struct(input: &DeriveInput, data: &syn::DataStruct) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    // Consume (and ignore) any #[config(prefix = "...")] for backwards compat
    consume_prefix_attr(input)?;
    let krate = r2e_core_path();

    let fields = match &data.fields {
        Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "#[derive(ConfigProperties)] only works on structs with named fields",
            ))
        }
    };

    // Parse all fields
    let mut field_infos = Vec::new();
    for field in fields {
        let field_name = field.ident.clone().unwrap();
        let field_type = field.ty.clone();
        let is_option = is_option_type(&field_type);
        let doc = extract_doc_comment(&field.attrs);
        let field_config = extract_field_config(&field.attrs)?;
        let (default_expr, default_str) = match field_config.default {
            Some((expr, s)) => {
                // String literals need .into() for &str → String conversion.
                // Other types (int, float, bool) work with direct assignment
                // via type inference from the target type annotation.
                let expr = if field_config.default_is_str_lit {
                    quote!(#expr.into())
                } else {
                    expr
                };
                (Some(expr), Some(s))
            }
            None => (None, None),
        };

        if field_config.skip
            && (field_config.section
                || field_config.default_flag
                || default_expr.is_some()
                || field_config.key.is_some()
                || field_config.env.is_some())
        {
            return Err(syn::Error::new_spanned(
                field,
                "#[config(skip)] cannot be combined with other #[config(...)] attributes",
            ));
        }
        if field_config.default_flag && !field_config.section {
            return Err(syn::Error::new_spanned(
                field,
                "#[config(default)] without a value is only supported on #[config(section)] fields \
                 (uses Default::default() when the section is absent)",
            ));
        }
        if field_config.default_flag && is_option {
            return Err(syn::Error::new_spanned(
                field,
                "#[config(section, default)] cannot be used on Option fields — \
                 an absent optional section is already None",
            ));
        }

        let is_map_section = field_config.section
            && string_map_value_type(if is_option {
                option_inner_type(&field_type).unwrap()
            } else {
                &field_type
            })
            .is_some();

        field_infos.push(FieldInfo {
            name: field_name,
            ty: field_type,
            default_expr,
            default_str,
            custom_key: field_config.key,
            is_option,
            doc,
            is_section: field_config.section,
            is_map_section,
            default_flag: field_config.default_flag,
            is_skip: field_config.skip,
            env_var: field_config.env,
            has_validate: has_validate_attr(&field.attrs),
        });
    }

    // Detect if any field has garde validation → we'll call validate() after construction
    let any_has_validate = field_infos.iter().any(|f| f.has_validate);

    // Generate metadata entries
    let metadata_entries: Vec<TokenStream2> = field_infos
        .iter()
        .filter(|f| !f.is_skip)
        .map(|f| {
            let key = match &f.custom_key {
                Some(k) => k.clone(),
                None => f.name.to_string(),
            };
            let type_name_simple = type_name_str(&f.ty);
            let required = !f.is_option && f.default_expr.is_none() && !f.is_section;
            let default_val = match &f.default_str {
                Some(s) => quote! { Some(#s.to_string()) },
                None => quote! { None },
            };
            let desc = match &f.doc {
                Some(d) => quote! { Some(#d.to_string()) },
                None => quote! { None },
            };
            let env_var_tok = match &f.env_var {
                Some(e) => quote! { Some(#e.to_string()) },
                None => quote! { None },
            };
            let is_section = f.is_section;
            let probe = resolvability_probe(f, &krate);
            quote! {
                {
                    let __full_key = match __prefix {
                        Some(__p) => format!("{}.{}", __p, #key),
                        None => #key.to_string(),
                    };
                    #krate::config::typed::PropertyMeta {
                        key: #key.to_string(),
                        full_key: __full_key,
                        type_name: #type_name_simple,
                        required: #required,
                        default_value: #default_val,
                        description: #desc,
                        env_var: #env_var_tok,
                        is_section: #is_section,
                        resolvable: #probe,
                    }
                }
            }
        })
        .collect();

    // Generate from_config field initializers
    let field_inits: Vec<TokenStream2> = field_infos
        .iter()
        .map(|f| {
            let field_name = &f.name;
            let key_str = match &f.custom_key {
                Some(k) => k.clone(),
                None => f.name.to_string(),
            };

            // Skipped fields are never read from config.
            if f.is_skip {
                return quote! {
                    #field_name: ::std::default::Default::default()
                };
            }

            // Section fields delegate to ConfigProperties::from_config
            if f.is_section {
                let section_prefix_expr = quote! {
                    match __prefix {
                        Some(__p) => format!("{}.{}", __p, #key_str),
                        None => #key_str.to_string(),
                    }
                };
                let section_ty = if f.is_option {
                    option_inner_type(&f.ty).unwrap()
                } else {
                    &f.ty
                };

                // Map-valued section: enumerate immediate sub-keys, parse each
                // entry as its own section. Absent prefix → empty map.
                if let Some(value_ty) = string_map_value_type(section_ty) {
                    let build_map = quote! {{
                        let mut __map: #section_ty = ::std::default::Default::default();
                        for __k in __config.sub_keys(&__section_prefix) {
                            let __child_prefix = format!("{}.{}", __section_prefix, __k);
                            let __v = <#value_ty as #krate::config::ConfigProperties>::from_config(
                                __config,
                                Some(&__child_prefix),
                            )?;
                            __map.insert(__k, __v);
                        }
                        __map
                    }};
                    if f.is_option {
                        return quote! {
                            #field_name: {
                                let __section_prefix = #section_prefix_expr;
                                if __config.has_prefix(&__section_prefix) {
                                    Some(#build_map)
                                } else {
                                    None
                                }
                            }
                        };
                    }
                    return quote! {
                        #field_name: {
                            let __section_prefix = #section_prefix_expr;
                            #build_map
                        }
                    };
                }

                let section_init = quote! {
                    <#section_ty as #krate::config::ConfigProperties>::from_config(
                        __config,
                        Some(&__section_prefix),
                    )
                };
                // Optional section: presence-based. None only when no key
                // lives under the prefix; a present-but-invalid section errors.
                if f.is_option {
                    return quote! {
                        #field_name: {
                            let __section_prefix = #section_prefix_expr;
                            if __config.has_prefix(&__section_prefix) {
                                Some(#section_init?)
                            } else {
                                None
                            }
                        }
                    };
                }
                // `#[config(section, default)]`: absent section → Default::default().
                if f.default_flag {
                    return quote! {
                        #field_name: {
                            let __section_prefix = #section_prefix_expr;
                            if __config.has_prefix(&__section_prefix) {
                                #section_init?
                            } else {
                                <#section_ty as ::std::default::Default>::default()
                            }
                        }
                    };
                }
                return quote! {
                    #field_name: {
                        let __section_prefix = #section_prefix_expr;
                        #section_init?
                    }
                };
            }

            // Build the key expression using runtime prefix
            let key_expr = quote! {
                &match __prefix {
                    Some(__p) => format!("{}.{}", __p, #key_str),
                    None => #key_str.to_string(),
                }
            };

            // Helper: generate env var fallback block that converts the
            // env string to the target type via FromConfigValue.
            let env_try = |_target_ty: &syn::Type, env_name: &str| {
                quote! {
                    match std::env::var(#env_name) {
                        Ok(__env_val) => {
                            let __cv = #krate::config::ConfigValue::String(__env_val);
                            match #krate::config::FromConfigValue::from_config_value(&__cv, __key) {
                                Ok(__v) => __v,
                                Err(__e) => return Err(__e),
                            }
                        }
                        Err(_) => return Err(#krate::config::ConfigError::NotFound(__key.to_string())),
                    }
                }
            };

            if f.is_option {
                let inner_ty = option_inner_type(&f.ty).unwrap();

                // Option<T> — env + default
                if let (Some(env_name), Some(default_expr)) = (&f.env_var, &f.default_expr) {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#inner_ty>(__key) {
                                Ok(v) => Some(v),
                                Err(#krate::config::ConfigError::NotFound(_)) => {
                                    match std::env::var(#env_name) {
                                        Ok(__env_val) => {
                                            let __cv = #krate::config::ConfigValue::String(__env_val);
                                            Some(#krate::config::FromConfigValue::from_config_value(&__cv, __key)?)
                                        }
                                        Err(_) => { let __d: #inner_ty = #default_expr; Some(__d) }
                                    }
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
                // Option<T> — env only
                else if let Some(env_name) = &f.env_var {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#inner_ty>(__key) {
                                Ok(v) => Some(v),
                                Err(#krate::config::ConfigError::NotFound(_)) => {
                                    match std::env::var(#env_name) {
                                        Ok(__env_val) => {
                                            let __cv = #krate::config::ConfigValue::String(__env_val);
                                            Some(#krate::config::FromConfigValue::from_config_value(&__cv, __key)?)
                                        }
                                        Err(_) => None,
                                    }
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
                // Option<T> — default only
                else if let Some(default_expr) = &f.default_expr {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#inner_ty>(__key) {
                                Ok(v) => Some(v),
                                Err(#krate::config::ConfigError::NotFound(_)) => { let __d: #inner_ty = #default_expr; Some(__d) }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
                // Option<T> — plain
                else {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#inner_ty>(__key) {
                                Ok(v) => Some(v),
                                Err(#krate::config::ConfigError::NotFound(_)) => None,
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
            } else if let Some(default_expr) = &f.default_expr {
                // Required with default — env + default
                let ty = &f.ty;
                if let Some(env_name) = &f.env_var {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#ty>(__key) {
                                Ok(v) => v,
                                Err(#krate::config::ConfigError::NotFound(_)) => {
                                    match std::env::var(#env_name) {
                                        Ok(__env_val) => {
                                            let __cv = #krate::config::ConfigValue::String(__env_val);
                                            #krate::config::FromConfigValue::from_config_value(&__cv, __key)?
                                        }
                                        Err(_) => { let __d: #ty = #default_expr; __d }
                                    }
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                } else {
                    // Required with default — no env
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#ty>(__key) {
                                Ok(v) => v,
                                Err(#krate::config::ConfigError::NotFound(_)) => { let __d: #ty = #default_expr; __d }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                }
            } else {
                // Required — no default
                let ty = &f.ty;
                if let Some(env_name) = &f.env_var {
                    let env_block = env_try(ty, env_name);
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            match __config.get::<#ty>(__key) {
                                Ok(v) => v,
                                Err(#krate::config::ConfigError::NotFound(_)) => { #env_block }
                                Err(e) => return Err(e),
                            }
                        }
                    }
                } else {
                    quote! {
                        #field_name: {
                            let __key: &str = #key_expr;
                            __config.get::<#ty>(__key)?
                        }
                    }
                }
            }
        })
        .collect();

    // Generate optional garde validation call
    let validation_call = if any_has_validate {
        quote! {
            {
                use garde::Validate as _;
                let __ctx = <#name as garde::Validate>::Context::default();
                let __prefix_str = __prefix.unwrap_or("");
                __instance.validate(&__ctx).map_err(|__report| {
                    let __details = __report.iter()
                        .map(|(path, error)| {
                            let __key = if __prefix_str.is_empty() {
                                format!("{}", path)
                            } else {
                                format!("{}.{}", __prefix_str, path)
                            };
                            #krate::config::ConfigValidationDetail {
                                key: __key,
                                message: error.message().to_string(),
                            }
                        })
                        .collect();
                    #krate::config::ConfigError::Validation(__details)
                })?;
            }
        }
    } else {
        quote! {}
    };

    // Generate register_children body for section fields.
    // Map-valued sections are excluded: all entries share one type, so
    // registering them as by-type beans would collide.
    let register_children_stmts: Vec<TokenStream2> = field_infos
        .iter()
        .filter(|f| f.is_section && !f.is_map_section)
        .map(|f| {
            let field_name = &f.name;
            if f.is_option {
                quote! {
                    if let Some(ref __child) = self.#field_name {
                        __registry.provide(__child.clone());
                        __child.register_children(__registry);
                    }
                }
            } else {
                quote! {
                    __registry.provide(self.#field_name.clone());
                    self.#field_name.register_children(__registry);
                }
            }
        })
        .collect();

    // Build `type Children` — a type-level list of all non-Option section types
    // plus their own Children, recursively flattened.
    //
    // For each non-Option section field of type `T`:
    //   TCons<T, <T as ConfigProperties>::Children as TAppend<...rest...>>::Output>
    //
    // For leaf configs (no sections), Children = TNil.
    let non_option_sections: Vec<&syn::Type> = field_infos
        .iter()
        .filter(|f| f.is_section && !f.is_option && !f.is_map_section)
        .map(|f| &f.ty)
        .collect();

    let children_type = if non_option_sections.is_empty() {
        quote! { #krate::type_list::TNil }
    } else {
        // Fold right-to-left: last section's children come first in the tail
        let mut acc = quote! { #krate::type_list::TNil };
        for ty in non_option_sections.iter().rev() {
            // TCons<T, <<T as ConfigProperties>::Children as TAppend<acc>>::Output>
            acc = quote! {
                #krate::type_list::TCons<
                    #ty,
                    <<#ty as #krate::config::ConfigProperties>::Children
                        as #krate::type_list::TAppend<#acc>>::Output
                >
            };
        }
        acc
    };

    Ok(quote! {
        impl #krate::config::typed::ConfigProperties for #name {
            type Children = #children_type;

            fn properties_metadata(__prefix: Option<&str>) -> Vec<#krate::config::typed::PropertyMeta> {
                vec![
                    #(#metadata_entries,)*
                ]
            }

            fn from_config(
                __config: &#krate::config::R2eConfig,
                __prefix: Option<&str>,
            ) -> Result<Self, #krate::config::ConfigError> {
                let __instance = Self {
                    #(#field_inits,)*
                };
                #validation_call
                Ok(__instance)
            }

            fn register_children(&self, __registry: &mut #krate::beans::BeanRegistry) {
                #(#register_children_stmts)*
            }
        }
    })
}

// ── Tagged enum support ─────────────────────────────────────────────────────

/// Enum-level `#[config(tag = "...", rename_all = "...")]` attributes.
struct EnumConfig {
    tag: Option<String>,
    rename_all: Option<(String, proc_macro2::Span)>,
}

fn extract_enum_config(attrs: &[syn::Attribute]) -> syn::Result<EnumConfig> {
    let mut result = EnumConfig {
        tag: None,
        rename_all: None,
    };
    for attr in attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("tag") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    result.tag = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("rename_all") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    result.rename_all = Some((lit.value(), lit.span()));
                    Ok(())
                } else {
                    Err(meta.error("expected `tag` or `rename_all` in #[config(...)] on an enum"))
                }
            })?;
        }
    }
    Ok(result)
}

/// Per-variant `#[config(rename = "...", default)]` attributes.
struct VariantConfig {
    rename: Option<String>,
    is_default: bool,
}

fn extract_variant_config(attrs: &[syn::Attribute]) -> syn::Result<VariantConfig> {
    let mut result = VariantConfig {
        rename: None,
        is_default: false,
    };
    for attr in attrs {
        if attr.path().is_ident("config") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    result.rename = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("default") {
                    result.is_default = true;
                    Ok(())
                } else {
                    Err(meta.error("expected `rename` or `default` in #[config(...)] on a variant"))
                }
            })?;
        }
    }
    Ok(result)
}

/// Split a PascalCase identifier into lowercase words joined by `delim`
/// (e.g. `PassThrough` → `pass_through`, `S3` → `s3`).
fn camel_to_delimited(s: &str, delim: char) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let prev_lower = chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit();
            let next_lower = chars.get(i + 1).map(|n| n.is_lowercase()).unwrap_or(false);
            if prev_lower || (chars[i - 1].is_uppercase() && next_lower) {
                out.push(delim);
            }
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// Apply a `rename_all` style to a variant identifier. Default: snake_case.
fn variant_tag_name(ident: &str, rename_all: Option<&(String, proc_macro2::Span)>) -> syn::Result<String> {
    match rename_all {
        None => Ok(camel_to_delimited(ident, '_')),
        Some((style, span)) => match style.as_str() {
            "snake_case" => Ok(camel_to_delimited(ident, '_')),
            "kebab-case" => Ok(camel_to_delimited(ident, '-')),
            "lowercase" => Ok(ident.to_lowercase()),
            other => Err(syn::Error::new(
                *span,
                format!(
                    "unsupported rename_all style `{other}` — expected \
                     `snake_case`, `kebab-case`, or `lowercase`"
                ),
            )),
        },
    }
}

/// Generate `ConfigProperties` for an internally-tagged enum.
///
/// ```ignore
/// #[derive(ConfigProperties, Clone)]
/// #[config(tag = "backend")]
/// pub enum StorageConfig {
///     S3(S3Config),
///     #[config(default)]
///     Filesystem(FilesystemConfig),
/// }
/// ```
///
/// The tag key is read relative to the prefix; the selected variant's payload
/// (if any) is parsed as a `ConfigProperties` section at the *same* prefix.
fn generate_enum(input: &DeriveInput, data: &syn::DataEnum) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let krate = r2e_core_path();

    let enum_config = extract_enum_config(&input.attrs)?;
    let Some(tag) = enum_config.tag else {
        return Err(syn::Error::new_spanned(
            name,
            "#[derive(ConfigProperties)] on an enum requires #[config(tag = \"...\")] — \
             the config key whose value selects the variant",
        ));
    };

    if data.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "#[derive(ConfigProperties)] requires at least one enum variant",
        ));
    }

    let mut tag_names: Vec<String> = Vec::new();
    let mut constructions: Vec<TokenStream2> = Vec::new();
    let mut default_construction: Option<TokenStream2> = None;
    let mut default_tag: Option<String> = None;

    for variant in &data.variants {
        let vident = &variant.ident;
        let vconfig = extract_variant_config(&variant.attrs)?;
        let tag_name = match vconfig.rename {
            Some(r) => r,
            None => variant_tag_name(&vident.to_string(), enum_config.rename_all.as_ref())?,
        };
        if tag_names.contains(&tag_name) {
            return Err(syn::Error::new_spanned(
                variant,
                format!("duplicate tag value `{tag_name}`"),
            ));
        }

        let construction = match &variant.fields {
            Fields::Unit => quote! { #name::#vident },
            Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => {
                let payload_ty = &unnamed.unnamed.first().unwrap().ty;
                quote! {
                    #name::#vident(
                        <#payload_ty as #krate::config::ConfigProperties>::from_config(
                            __config,
                            __prefix,
                        )?,
                    )
                }
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    variant,
                    "tagged-enum variants must be unit variants or single-field tuple \
                     variants whose field implements ConfigProperties",
                ))
            }
        };

        if vconfig.is_default {
            if default_construction.is_some() {
                return Err(syn::Error::new_spanned(
                    variant,
                    "only one variant may be marked #[config(default)]",
                ));
            }
            default_construction = Some(construction.clone());
            default_tag = Some(tag_name.clone());
        }

        tag_names.push(tag_name);
        constructions.push(construction);
    }

    let expected_list = tag_names.join(", ");
    let none_arm = match &default_construction {
        Some(construction) => quote! { Ok(#construction) },
        None => quote! { Err(#krate::config::ConfigError::NotFound(__tag_key)) },
    };
    let tag_required = default_construction.is_none();
    let default_tag_tok = match &default_tag {
        Some(t) => quote! { Some(#t.to_string()) },
        None => quote! { None },
    };
    let tag_description = format!("one of: {expected_list}");

    Ok(quote! {
        impl #krate::config::typed::ConfigProperties for #name {
            type Children = #krate::type_list::TNil;

            fn properties_metadata(__prefix: Option<&str>) -> Vec<#krate::config::typed::PropertyMeta> {
                let __full_key = match __prefix {
                    Some(__p) => format!("{}.{}", __p, #tag),
                    None => #tag.to_string(),
                };
                vec![#krate::config::typed::PropertyMeta {
                    key: #tag.to_string(),
                    full_key: __full_key,
                    type_name: "String",
                    required: #tag_required,
                    default_value: #default_tag_tok,
                    description: Some(#tag_description.to_string()),
                    env_var: None,
                    is_section: false,
                    // env_var is None, so this probes the config map only —
                    // matching from_config, which reads the tag from the map.
                    resolvable: #krate::config::typed::PropertyMeta::standard_sources,
                }]
            }

            fn from_config(
                __config: &#krate::config::R2eConfig,
                __prefix: Option<&str>,
            ) -> Result<Self, #krate::config::ConfigError> {
                let __tag_key = match __prefix {
                    Some(__p) => format!("{}.{}", __p, #tag),
                    None => #tag.to_string(),
                };
                let __tag_val: Option<String> = match __config.get::<String>(&__tag_key) {
                    Ok(v) => Some(v),
                    Err(#krate::config::ConfigError::NotFound(_)) => None,
                    Err(e) => return Err(e),
                };
                match __tag_val.as_deref() {
                    #( Some(#tag_names) => Ok(#constructions), )*
                    Some(__other) => Err(#krate::config::ConfigError::Deserialize {
                        key: __tag_key,
                        message: format!(
                            "unknown tag value `{}` — expected one of: {}",
                            __other, #expected_list,
                        ),
                    }),
                    None => #none_arm,
                }
            }
        }
    })
}

/// Get a simple type name string for metadata.
fn type_name_str(ty: &syn::Type) -> String {
    if let syn::Type::Path(syn::TypePath { path, .. }) = ty {
        if let Some(seg) = path.segments.last() {
            let name = seg.ident.to_string();
            if name == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return format!("Option<{}>", type_name_str(inner));
                    }
                }
            }
            if name == "Vec" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return format!("Vec<{}>", type_name_str(inner));
                    }
                }
            }
            return name;
        }
    }
    quote!(#ty).to_string()
}
