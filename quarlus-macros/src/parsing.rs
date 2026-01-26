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
    pub transactional: bool,
    pub logged: bool,
    pub timed: bool,
    pub cached: Option<u64>,
    pub rate_limited: Option<RateLimitConfig>,
    pub middleware_fns: Vec<syn::Path>,
    pub fn_item: syn::ImplItemFn,
}

pub struct RateLimitConfig {
    pub max: u64,
    pub window: u64,
}

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
                        let transactional = all_attrs.iter().any(|a| a.path().is_ident("transactional"));
                        let logged = all_attrs.iter().any(|a| a.path().is_ident("logged"));
                        let timed = all_attrs.iter().any(|a| a.path().is_ident("timed"));
                        let cached = extract_cached_ttl(&all_attrs)?;
                        let rate_limited = extract_rate_limit(&all_attrs)?;
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

fn extract_cached_ttl(attrs: &[syn::Attribute]) -> syn::Result<Option<u64>> {
    for attr in attrs {
        if attr.path().is_ident("cached") {
            let mut ttl = 60u64;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("ttl") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    ttl = lit.base10_parse()?;
                    Ok(())
                } else {
                    Err(meta.error("expected `ttl`"))
                }
            })?;
            return Ok(Some(ttl));
        }
    }
    Ok(None)
}

fn extract_rate_limit(attrs: &[syn::Attribute]) -> syn::Result<Option<RateLimitConfig>> {
    for attr in attrs {
        if attr.path().is_ident("rate_limited") {
            let mut max = 100u64;
            let mut window = 60u64;
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
                } else {
                    Err(meta.error("expected `max` or `window`"))
                }
            })?;
            return Ok(Some(RateLimitConfig { max, window }));
        }
    }
    Ok(None)
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
