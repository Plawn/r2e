//! Route-related attribute extraction.

use quote::quote;

use crate::crate_path::r2e_security_path;
use crate::route::{HttpMethod, RoutePath};
use crate::types::TransactionalConfig;

pub fn is_route_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("get")
        || attr.path().is_ident("post")
        || attr.path().is_ident("put")
        || attr.path().is_ident("delete")
        || attr.path().is_ident("patch")
}

pub fn is_sse_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("sse")
}

pub fn is_ws_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("ws")
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

pub fn roles_guard_expr(roles: &[String]) -> Option<syn::Expr> {
    if roles.is_empty() {
        return None;
    }
    let krate = r2e_security_path();
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

/// Extract `#[sse("/path")]` or `#[sse("/path", keep_alive = ...)]`.
/// Returns `(path, keep_alive)` if found.
pub fn extract_sse_attr(attrs: &[syn::Attribute]) -> syn::Result<Option<(String, crate::types::SseKeepAlive)>> {
    for attr in attrs {
        if !attr.path().is_ident("sse") {
            continue;
        }
        // Parse arguments: first is the path, optionally followed by keep_alive = ...
        let args: syn::punctuated::Punctuated<syn::Expr, syn::Token![,]> =
            attr.parse_args_with(syn::punctuated::Punctuated::parse_terminated)?;

        let mut iter = args.iter();
        let path = match iter.next() {
            Some(syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(lit_str),
                ..
            })) => lit_str.value(),
            _ => return Err(syn::Error::new_spanned(attr, "#[sse] requires a path string as first argument")),
        };

        let mut keep_alive = crate::types::SseKeepAlive::Default;

        for expr in iter {
            // Parse keep_alive = <value>
            if let syn::Expr::Assign(assign) = expr {
                if let syn::Expr::Path(ref p) = *assign.left {
                    if p.path.is_ident("keep_alive") {
                        match *assign.right.clone() {
                            syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Bool(ref b),
                                ..
                            }) => {
                                if !b.value {
                                    keep_alive = crate::types::SseKeepAlive::Disabled;
                                }
                            }
                            syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Int(ref i),
                                ..
                            }) => {
                                keep_alive = crate::types::SseKeepAlive::Interval(i.base10_parse()?);
                            }
                            _ => return Err(syn::Error::new_spanned(
                                &assign.right,
                                "keep_alive must be a bool or integer",
                            )),
                        }
                        continue;
                    }
                }
            }
            return Err(syn::Error::new_spanned(expr, "unexpected argument in #[sse]"));
        }

        return Ok(Some((path, keep_alive)));
    }
    Ok(None)
}

/// Extract `#[status(N)]` — override default HTTP status code.
pub fn extract_status(attrs: &[syn::Attribute]) -> syn::Result<Option<u16>> {
    for attr in attrs {
        if attr.path().is_ident("status") {
            let lit: syn::LitInt = attr.parse_args()?;
            return Ok(Some(lit.base10_parse()?));
        }
    }
    Ok(None)
}

/// Extract `#[returns(T)]` — explicit response type for custom wrappers.
pub fn extract_returns(attrs: &[syn::Attribute]) -> syn::Result<Option<syn::Type>> {
    for attr in attrs {
        if attr.path().is_ident("returns") {
            let ty: syn::Type = attr.parse_args()?;
            return Ok(Some(ty));
        }
    }
    Ok(None)
}

/// Extract `///` doc comments from attributes.
/// Rustc desugars `/// text` into `#[doc = "text"]`.
/// Returns (summary, description): first non-empty line → summary, remaining → description.
pub fn extract_doc_comments(attrs: &[syn::Attribute]) -> (Option<String>, Option<String>) {
    let doc_lines: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if attr.path().is_ident("doc") {
                if let syn::Meta::NameValue(nv) = &attr.meta {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(lit_str),
                        ..
                    }) = &nv.value
                    {
                        return Some(lit_str.value());
                    }
                }
            }
            None
        })
        .collect();

    if doc_lines.is_empty() {
        return (None, None);
    }

    // First non-empty trimmed line is the summary
    let mut summary = None;
    let mut desc_start = 0;
    for (i, line) in doc_lines.iter().enumerate() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            summary = Some(trimmed.to_string());
            desc_start = i + 1;
            break;
        }
    }

    // Skip empty lines between summary and description
    while desc_start < doc_lines.len() && doc_lines[desc_start].trim().is_empty() {
        desc_start += 1;
    }

    // Remaining non-empty lines form the description
    let desc_lines: Vec<&str> = doc_lines[desc_start..]
        .iter()
        .map(|l| l.trim())
        .collect();
    let description = if desc_lines.is_empty() || desc_lines.iter().all(|l| l.is_empty()) {
        None
    } else {
        Some(desc_lines.join("\n").trim().to_string())
    };

    (summary, description)
}

/// Detect standard `#[deprecated]` attribute.
pub fn is_deprecated(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("deprecated"))
}

/// Extract `#[ws("/path")]`. Returns the path if found.
pub fn extract_ws_attr(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if !attr.path().is_ident("ws") {
            continue;
        }
        let route_path: RoutePath = attr.parse_args()?;
        return Ok(Some(route_path.path));
    }
    Ok(None)
}
