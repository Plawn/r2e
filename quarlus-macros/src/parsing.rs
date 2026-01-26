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
    pub injected_fields: Vec<InjectedField>,
    pub identity_fields: Vec<IdentityField>,
    pub config_fields: Vec<ConfigField>,
    pub route_methods: Vec<RouteMethod>,
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

pub struct RouteMethod {
    pub method: HttpMethod,
    pub path: String,
    pub roles: Vec<String>,
    pub transactional: Option<TransactionalConfig>,
    pub logged: Option<LoggedConfig>,
    pub timed: Option<TimedConfig>,
    pub cached: Option<CachedConfig>,
    pub rate_limited: Option<RateLimitConfig>,
    pub cache_invalidate: Vec<String>,
    pub intercept_fns: Vec<syn::Path>,
    pub middleware_fns: Vec<syn::Path>,
    pub fn_item: syn::ImplItemFn,
}

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

pub struct LoggedConfig {
    pub level: LogLevel,
}

pub struct TimedConfig {
    pub level: LogLevel,
    pub threshold_ms: Option<u64>,
}

pub struct CachedConfig {
    pub ttl: u64,
    pub key: CacheKey,
    pub group: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheKey {
    Default,
    Params,
    User,
    UserParams,
}

pub struct TransactionalConfig {
    pub pool_field: String,
}

pub struct RateLimitConfig {
    pub max: u64,
    pub window: u64,
    pub key: RateLimitKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RateLimitKey {
    Global,
    User,
    Ip,
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

impl Parse for ControllerDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
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
        let mut other_methods = Vec::new();

        while !content.is_empty() {
            let attrs = content.call(syn::Attribute::parse_outer)?;

            if is_method_ahead(&content) {
                let mut method: syn::ImplItemFn = content.parse()?;

                // Merge pre-parsed attrs with any attrs syn parsed
                let mut all_attrs = attrs;
                all_attrs.append(&mut method.attrs);

                match extract_route_attr(&all_attrs)? {
                    Some((http_method, path)) => {
                        let roles = extract_roles(&all_attrs)?;
                        let transactional = extract_transactional(&all_attrs)?;
                        let logged = extract_logged(&all_attrs)?;
                        let timed = extract_timed(&all_attrs)?;
                        let cached = extract_cached(&all_attrs)?;
                        let rate_limited = extract_rate_limit(&all_attrs)?;
                        let cache_invalidate = extract_cache_invalidate(&all_attrs)?;
                        let intercept_fns = extract_intercept_fns(&all_attrs)?;
                        let middleware_fns = extract_middleware_fns(&all_attrs)?;
                        method.attrs = strip_route_attrs(all_attrs);
                        route_methods.push(RouteMethod {
                            method: http_method,
                            path,
                            roles,
                            transactional,
                            logged,
                            timed,
                            cached,
                            rate_limited,
                            cache_invalidate,
                            intercept_fns,
                            middleware_fns,
                            fn_item: method,
                        });
                    }
                    None => {
                        method.attrs = all_attrs;
                        other_methods.push(method);
                    }
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

        Ok(ControllerDef {
            name,
            state_type,
            injected_fields,
            identity_fields,
            config_fields,
            route_methods,
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
                && !a.path().is_ident("logged")
                && !a.path().is_ident("timed")
                && !a.path().is_ident("cached")
                && !a.path().is_ident("rate_limited")
                && !a.path().is_ident("cache_invalidate")
                && !a.path().is_ident("intercept")
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

fn extract_logged(attrs: &[syn::Attribute]) -> syn::Result<Option<LoggedConfig>> {
    for attr in attrs {
        if attr.path().is_ident("logged") {
            let mut level = LogLevel::Info;
            // Bare #[logged] → no args, use default Info
            // #[logged(level = "debug")] → parse args
            if matches!(attr.meta, syn::Meta::List(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("level") {
                        let value = meta.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        level = LogLevel::from_str(&lit.value()).ok_or_else(|| {
                            meta.error("expected one of: trace, debug, info, warn, error")
                        })?;
                        Ok(())
                    } else {
                        Err(meta.error("expected `level`"))
                    }
                })?;
            }
            return Ok(Some(LoggedConfig { level }));
        }
    }
    Ok(None)
}

fn extract_timed(attrs: &[syn::Attribute]) -> syn::Result<Option<TimedConfig>> {
    for attr in attrs {
        if attr.path().is_ident("timed") {
            let mut level = LogLevel::Info;
            let mut threshold_ms = None;
            // Bare #[timed] → use defaults
            // #[timed(level = "warn", threshold = 100)] → parse args
            if matches!(attr.meta, syn::Meta::List(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("level") {
                        let value = meta.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        level = LogLevel::from_str(&lit.value()).ok_or_else(|| {
                            meta.error("expected one of: trace, debug, info, warn, error")
                        })?;
                        Ok(())
                    } else if meta.path.is_ident("threshold") {
                        let value = meta.value()?;
                        let lit: syn::LitInt = value.parse()?;
                        threshold_ms = Some(lit.base10_parse::<u64>()?);
                        Ok(())
                    } else {
                        Err(meta.error("expected `level` or `threshold`"))
                    }
                })?;
            }
            return Ok(Some(TimedConfig {
                level,
                threshold_ms,
            }));
        }
    }
    Ok(None)
}

fn extract_cached(attrs: &[syn::Attribute]) -> syn::Result<Option<CachedConfig>> {
    for attr in attrs {
        if attr.path().is_ident("cached") {
            let mut ttl = 60u64;
            let mut key = CacheKey::Default;
            let mut group = None;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("ttl") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    ttl = lit.base10_parse()?;
                    Ok(())
                } else if meta.path.is_ident("key") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    key = match lit.value().as_str() {
                        "default" => CacheKey::Default,
                        "params" => CacheKey::Params,
                        "user" => CacheKey::User,
                        "user_params" => CacheKey::UserParams,
                        _ => return Err(meta.error("expected one of: default, params, user, user_params")),
                    };
                    Ok(())
                } else if meta.path.is_ident("group") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    group = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `ttl`, `key`, or `group`"))
                }
            })?;
            return Ok(Some(CachedConfig { ttl, key, group }));
        }
    }
    Ok(None)
}

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

fn extract_rate_limit(attrs: &[syn::Attribute]) -> syn::Result<Option<RateLimitConfig>> {
    for attr in attrs {
        if attr.path().is_ident("rate_limited") {
            let mut max = 100u64;
            let mut window = 60u64;
            let mut key = RateLimitKey::Global;
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
                    key = match lit.value().as_str() {
                        "global" => RateLimitKey::Global,
                        "user" => RateLimitKey::User,
                        "ip" => RateLimitKey::Ip,
                        _ => return Err(meta.error("expected one of: global, user, ip")),
                    };
                    Ok(())
                } else {
                    Err(meta.error("expected `max`, `window`, or `key`"))
                }
            })?;
            return Ok(Some(RateLimitConfig { max, window, key }));
        }
    }
    Ok(None)
}

fn extract_cache_invalidate(attrs: &[syn::Attribute]) -> syn::Result<Vec<String>> {
    let mut groups = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("cache_invalidate") {
            let lit: syn::LitStr = attr.parse_args()?;
            groups.push(lit.value());
        }
    }
    Ok(groups)
}

fn extract_intercept_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Path>> {
    let mut fns = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("intercept") {
            let path: syn::Path = attr.parse_args()?;
            fns.push(path);
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
