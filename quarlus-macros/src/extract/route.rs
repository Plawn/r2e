//! Route-related attribute extraction.

use quote::quote;

use crate::crate_path::{quarlus_core_path, quarlus_rate_limit_path};
use crate::route::{HttpMethod, RoutePath};
use crate::types::TransactionalConfig;

pub fn is_route_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("get")
        || attr.path().is_ident("post")
        || attr.path().is_ident("put")
        || attr.path().is_ident("delete")
        || attr.path().is_ident("patch")
}

pub fn strip_route_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| {
            !is_route_attr(a)
                && !a.path().is_ident("roles")
                && !a.path().is_ident("transactional")
                && !a.path().is_ident("rate_limited")
                && !a.path().is_ident("intercept")
                && !a.path().is_ident("guard")
                && !a.path().is_ident("pre_guard")
                && !a.path().is_ident("middleware")
                && !a.path().is_ident("layer")
        })
        .collect()
}

pub fn extract_route_attr(attrs: &[syn::Attribute]) -> syn::Result<Option<(HttpMethod, String)>> {
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

pub fn extract_roles(attrs: &[syn::Attribute]) -> syn::Result<Vec<String>> {
    for attr in attrs {
        if attr.path().is_ident("roles") {
            let args: syn::punctuated::Punctuated<syn::LitStr, syn::Token![,]> =
                attr.parse_args_with(syn::punctuated::Punctuated::parse_terminated)?;
            return Ok(args.iter().map(|lit| lit.value()).collect());
        }
    }
    Ok(Vec::new())
}

pub fn extract_transactional(attrs: &[syn::Attribute]) -> syn::Result<Option<TransactionalConfig>> {
    for attr in attrs {
        if attr.path().is_ident("transactional") {
            let mut pool_field = "pool".to_string();
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

/// Result of parsing `#[rate_limited]` — classified into pre-auth or post-auth.
pub struct RateLimitedGuards {
    /// Post-auth guard (for user-keyed rate limiting).
    pub post_auth: Option<syn::Expr>,
    /// Pre-auth guard (for global or IP-keyed rate limiting).
    pub pre_auth: Option<syn::Expr>,
}

pub fn extract_rate_limited_guards(attrs: &[syn::Attribute]) -> syn::Result<RateLimitedGuards> {
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

            let rl_krate = quarlus_rate_limit_path();

            let key_kind_tokens = match key_str.as_str() {
                "global" => quote! { #rl_krate::RateLimitKeyKind::Global },
                "user" => quote! { #rl_krate::RateLimitKeyKind::User },
                "ip" => quote! { #rl_krate::RateLimitKeyKind::Ip },
                _ => {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "expected one of: global, user, ip",
                    ))
                }
            };

            return match key_str.as_str() {
                "user" => {
                    // User-keyed → post-auth (needs identity)
                    let expr_tokens = quote! {
                        #rl_krate::RateLimitGuard {
                            max: #max,
                            window_secs: #window,
                            key: #key_kind_tokens,
                        }
                    };
                    let expr: syn::Expr = syn::parse2(expr_tokens)?;
                    Ok(RateLimitedGuards { post_auth: Some(expr), pre_auth: None })
                }
                _ => {
                    // Global or IP → pre-auth
                    let expr_tokens = quote! {
                        #rl_krate::PreAuthRateLimitGuard {
                            max: #max,
                            window_secs: #window,
                            key: #key_kind_tokens,
                        }
                    };
                    let expr: syn::Expr = syn::parse2(expr_tokens)?;
                    Ok(RateLimitedGuards { post_auth: None, pre_auth: Some(expr) })
                }
            };
        }
    }
    Ok(RateLimitedGuards { post_auth: None, pre_auth: None })
}

pub fn roles_guard_expr(roles: &[String]) -> Option<syn::Expr> {
    if roles.is_empty() {
        return None;
    }
    let krate = quarlus_core_path();
    let tokens = quote! {
        #krate::RolesGuard {
            required_roles: &[#(#roles),*],
        }
    };
    syn::parse2(tokens).ok()
}

/// Macro to extract multiple attributes by name and parse their arguments.
/// The name is inlined at compile time.
macro_rules! extract_attrs_by_name {
    ($attrs:expr, $name:literal) => {
        $attrs
            .iter()
            .filter(|a| a.path().is_ident($name))
            .map(|a| a.parse_args())
            .collect()
    };
}

pub fn extract_intercept_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    extract_attrs_by_name!(attrs, "intercept")
}

pub fn extract_guard_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    extract_attrs_by_name!(attrs, "guard")
}

pub fn extract_middleware_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Path>> {
    extract_attrs_by_name!(attrs, "middleware")
}

pub fn extract_layer_exprs(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    extract_attrs_by_name!(attrs, "layer")
}

pub fn extract_pre_guard_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    extract_attrs_by_name!(attrs, "pre_guard")
}
