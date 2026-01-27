use syn::parse::{Parse, ParseStream};
use syn::Token;

use crate::route::{HttpMethod, RoutePath};

/// Parsed representation of the `controller!` macro input:
///
/// ```ignore
/// controller! {
///     impl HelloController for Services {
///         #[inject]
///         greeting: String,
///
///         #[get("/hello")]
///         async fn hello(&self) -> String { ... }
///     }
/// }
/// ```
pub struct ControllerDef {
    pub name: syn::Ident,
    pub state_type: syn::Path,
    pub prefix: Option<String>,
    pub controller_intercepts: Vec<syn::Expr>,
    pub injected_fields: Vec<InjectedField>,
    pub identity_fields: Vec<IdentityField>,
    pub config_fields: Vec<ConfigField>,
    pub route_methods: Vec<RouteMethod>,
    pub consumer_methods: Vec<ConsumerMethod>,
    pub other_methods: Vec<syn::ImplItemFn>,
}

pub struct InjectedField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct IdentityField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct ConfigField {
    pub name: syn::Ident,
    pub ty: syn::Type,
    pub key: String,
}

pub struct ConsumerMethod {
    pub bus_field: String,
    pub event_type: syn::Type,
    pub fn_item: syn::ImplItemFn,
}

pub struct RouteMethod {
    pub method: HttpMethod,
    pub path: String,
    pub roles: Vec<String>,
    pub transactional: Option<TransactionalConfig>,
    pub intercept_fns: Vec<syn::Expr>,
    pub guard_fns: Vec<syn::Expr>,
    pub middleware_fns: Vec<syn::Path>,
    pub fn_item: syn::ImplItemFn,
}

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

pub struct TransactionalConfig {
    pub pool_field: String,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

impl Parse for ControllerDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Parse optional outer attributes (before `impl`), e.g. #[path("/users")]
        let outer_attrs = input.call(syn::Attribute::parse_outer)?;
        let prefix = extract_path_prefix(&outer_attrs)?;
        let controller_intercepts = extract_intercept_fns(&outer_attrs)?;

        // impl Name for StatePath { ... }
        let _: Token![impl] = input.parse()?;
        let name: syn::Ident = input.parse()?;
        let _: Token![for] = input.parse()?;
        let state_type: syn::Path = input.parse()?;

        let content;
        syn::braced!(content in input);

        let mut injected_fields = Vec::new();
        let mut identity_fields = Vec::new();
        let mut config_fields = Vec::new();
        let mut route_methods = Vec::new();
        let mut consumer_methods = Vec::new();
        let mut other_methods = Vec::new();

        while !content.is_empty() {
            let attrs = content.call(syn::Attribute::parse_outer)?;

            if is_method_ahead(&content) {
                let mut method: syn::ImplItemFn = content.parse()?;

                // Merge pre-parsed attrs with any attrs syn parsed
                let mut all_attrs = attrs;
                all_attrs.append(&mut method.attrs);

                if let Some(bus_field) = extract_consumer(&all_attrs)? {
                    let event_param = method
                        .sig
                        .inputs
                        .iter()
                        .find_map(|arg| match arg {
                            syn::FnArg::Typed(pt) => Some(pt),
                            _ => None,
                        })
                        .ok_or_else(|| {
                            syn::Error::new(
                                method.sig.ident.span(),
                                "consumer method must have an event parameter",
                            )
                        })?;
                    let event_type = extract_event_type_from_arc(&event_param.ty)?;
                    method.attrs = strip_consumer_attrs(all_attrs);
                    consumer_methods.push(ConsumerMethod {
                        bus_field,
                        event_type,
                        fn_item: method,
                    });
                } else if let Some((http_method, path)) = extract_route_attr(&all_attrs)? {
                    let roles = extract_roles(&all_attrs)?;
                    let transactional = extract_transactional(&all_attrs)?;
                    let intercept_fns = extract_intercept_fns(&all_attrs)?;
                    let middleware_fns = extract_middleware_fns(&all_attrs)?;

                    // Build guard_fns: rate_limited -> roles -> custom guards
                    let mut guard_fns = Vec::new();
                    if let Some(rl_guard) = extract_rate_limited_guard(&all_attrs)? {
                        guard_fns.push(rl_guard);
                    }
                    if let Some(roles_guard) = roles_guard_expr(&roles) {
                        guard_fns.push(roles_guard);
                    }
                    guard_fns.extend(extract_guard_fns(&all_attrs)?);

                    method.attrs = strip_route_attrs(all_attrs);
                    route_methods.push(RouteMethod {
                        method: http_method,
                        path,
                        roles,
                        transactional,
                        intercept_fns,
                        guard_fns,
                        middleware_fns,
                        fn_item: method,
                    });
                } else {
                    method.attrs = all_attrs;
                    other_methods.push(method);
                }
            } else {
                // Field declaration: name: Type,
                let field_name: syn::Ident = content.parse()?;
                let _: Token![:] = content.parse()?;
                let field_type: syn::Type = content.parse()?;
                if content.peek(Token![,]) {
                    let _: Token![,] = content.parse()?;
                }

                let is_inject = attrs.iter().any(|a| a.path().is_ident("inject"));
                let is_identity = attrs.iter().any(|a| a.path().is_ident("identity"));
                let config_attr = attrs.iter().find(|a| a.path().is_ident("config"));

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
        }

        if !consumer_methods.is_empty() && !identity_fields.is_empty() {
            return Err(syn::Error::new(
                name.span(),
                "controllers with #[consumer] methods cannot have #[identity] fields \
                 (no HTTP request context available for event consumers)",
            ));
        }

        Ok(ControllerDef {
            name,
            state_type,
            prefix,
            controller_intercepts,
            injected_fields,
            identity_fields,
            config_fields,
            route_methods,
            consumer_methods,
            other_methods,
        })
    }
}

fn is_method_ahead(input: ParseStream) -> bool {
    input.peek(Token![pub])
        || input.peek(Token![async])
        || input.peek(Token![fn])
        || input.peek(Token![unsafe])
}

fn is_route_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("get")
        || attr.path().is_ident("post")
        || attr.path().is_ident("put")
        || attr.path().is_ident("delete")
        || attr.path().is_ident("patch")
}

fn strip_route_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| {
            !is_route_attr(a)
                && !a.path().is_ident("roles")
                && !a.path().is_ident("transactional")
                && !a.path().is_ident("rate_limited")
                && !a.path().is_ident("intercept")
                && !a.path().is_ident("guard")
                && !a.path().is_ident("middleware")
        })
        .collect()
}

fn extract_roles(attrs: &[syn::Attribute]) -> syn::Result<Vec<String>> {
    for attr in attrs {
        if attr.path().is_ident("roles") {
            let args: syn::punctuated::Punctuated<syn::LitStr, syn::Token![,]> =
                attr.parse_args_with(syn::punctuated::Punctuated::parse_terminated)?;
            return Ok(args.iter().map(|lit| lit.value()).collect());
        }
    }
    Ok(Vec::new())
}

// ---------------------------------------------------------------------------
// Extended attribute extraction
// ---------------------------------------------------------------------------

fn extract_transactional(attrs: &[syn::Attribute]) -> syn::Result<Option<TransactionalConfig>> {
    for attr in attrs {
        if attr.path().is_ident("transactional") {
            let mut pool_field = "pool".to_string();
            // Bare #[transactional] → use default pool "pool"
            // #[transactional(pool = "read_db")] → custom pool field
            if matches!(attr.meta, syn::Meta::List(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("pool") {
                        let value = meta.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        pool_field = lit.value();
                        Ok(())
                    } else {
                        Err(meta.error("expected `pool`"))
                    }
                })?;
            }
            return Ok(Some(TransactionalConfig { pool_field }));
        }
    }
    Ok(None)
}

fn extract_rate_limited_guard(attrs: &[syn::Attribute]) -> syn::Result<Option<syn::Expr>> {
    for attr in attrs {
        if attr.path().is_ident("rate_limited") {
            let mut max: u64 = 100;
            let mut window: u64 = 60;
            let mut key_str = String::from("global");
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("max") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    max = lit.base10_parse()?;
                    Ok(())
                } else if meta.path.is_ident("window") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    window = lit.base10_parse()?;
                    Ok(())
                } else if meta.path.is_ident("key") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    key_str = lit.value();
                    Ok(())
                } else {
                    Err(meta.error("expected `max`, `window`, or `key`"))
                }
            })?;
            let key_kind: syn::Expr = match key_str.as_str() {
                "global" => syn::parse_quote! { quarlus_rate_limit::RateLimitKeyKind::Global },
                "user" => syn::parse_quote! { quarlus_rate_limit::RateLimitKeyKind::User },
                "ip" => syn::parse_quote! { quarlus_rate_limit::RateLimitKeyKind::Ip },
                _ => {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "expected one of: global, user, ip",
                    ))
                }
            };
            let expr: syn::Expr = syn::parse_quote! {
                quarlus_rate_limit::RateLimitGuard {
                    max: #max,
                    window_secs: #window,
                    key: #key_kind,
                }
            };
            return Ok(Some(expr));
        }
    }
    Ok(None)
}

fn roles_guard_expr(roles: &[String]) -> Option<syn::Expr> {
    if roles.is_empty() {
        return None;
    }
    Some(syn::parse_quote! {
        quarlus_core::RolesGuard {
            required_roles: &[#(#roles),*],
        }
    })
}

fn extract_intercept_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    let mut fns = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("intercept") {
            let expr: syn::Expr = attr.parse_args()?;
            fns.push(expr);
        }
    }
    Ok(fns)
}

fn extract_guard_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    let mut fns = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("guard") {
            let expr: syn::Expr = attr.parse_args()?;
            fns.push(expr);
        }
    }
    Ok(fns)
}

fn extract_middleware_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Path>> {
    let mut fns = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("middleware") {
            let path: syn::Path = attr.parse_args()?;
            fns.push(path);
        }
    }
    Ok(fns)
}

fn extract_route_attr(attrs: &[syn::Attribute]) -> syn::Result<Option<(HttpMethod, String)>> {
    for attr in attrs {
        let method = if attr.path().is_ident("get") {
            Some(HttpMethod::Get)
        } else if attr.path().is_ident("post") {
            Some(HttpMethod::Post)
        } else if attr.path().is_ident("put") {
            Some(HttpMethod::Put)
        } else if attr.path().is_ident("delete") {
            Some(HttpMethod::Delete)
        } else if attr.path().is_ident("patch") {
            Some(HttpMethod::Patch)
        } else {
            None
        };

        if let Some(method) = method {
            let route_path: RoutePath = attr.parse_args()?;
            return Ok(Some((method, route_path.path)));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Consumer attribute extraction
// ---------------------------------------------------------------------------

fn extract_consumer(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if attr.path().is_ident("consumer") {
            let mut bus_field = String::new();
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("bus") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    bus_field = lit.value();
                    Ok(())
                } else {
                    Err(meta.error("expected `bus`"))
                }
            })?;
            if bus_field.is_empty() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[consumer] requires bus = \"field_name\"",
                ));
            }
            return Ok(Some(bus_field));
        }
    }
    Ok(None)
}

fn extract_event_type_from_arc(ty: &syn::Type) -> syn::Result<syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Arc" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Ok(inner.clone());
                    }
                }
            }
        }
    }
    Err(syn::Error::new_spanned(
        ty,
        "consumer parameter must be Arc<EventType>",
    ))
}

fn strip_consumer_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("consumer"))
        .collect()
}

// ---------------------------------------------------------------------------
// Path prefix extraction
// ---------------------------------------------------------------------------

fn extract_path_prefix(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if attr.path().is_ident("path") {
            let route_path: RoutePath = attr.parse_args()?;
            return Ok(Some(route_path.path));
        }
    }
    Ok(None)
}
