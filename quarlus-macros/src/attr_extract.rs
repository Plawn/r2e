use crate::route::{HttpMethod, RoutePath};
use crate::types::*;
use syn::spanned::Spanned;

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
                && !a.path().is_ident("middleware")
                && !a.path().is_ident("layer")
        })
        .collect()
}

pub fn strip_consumer_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("consumer"))
        .collect()
}

pub fn strip_scheduled_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("scheduled") && !a.path().is_ident("intercept"))
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

pub fn extract_rate_limited_guard(attrs: &[syn::Attribute]) -> syn::Result<Option<syn::Expr>> {
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

pub fn roles_guard_expr(roles: &[String]) -> Option<syn::Expr> {
    if roles.is_empty() {
        return None;
    }
    Some(syn::parse_quote! {
        quarlus_core::RolesGuard {
            required_roles: &[#(#roles),*],
        }
    })
}

pub fn extract_intercept_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    let mut fns = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("intercept") {
            let expr: syn::Expr = attr.parse_args()?;
            fns.push(expr);
        }
    }
    Ok(fns)
}

pub fn extract_guard_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    let mut fns = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("guard") {
            let expr: syn::Expr = attr.parse_args()?;
            fns.push(expr);
        }
    }
    Ok(fns)
}

pub fn extract_middleware_fns(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Path>> {
    let mut fns = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("middleware") {
            let path: syn::Path = attr.parse_args()?;
            fns.push(path);
        }
    }
    Ok(fns)
}

pub fn extract_consumer(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
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

pub fn extract_event_type_from_arc(ty: &syn::Type) -> syn::Result<syn::Type> {
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

pub fn extract_layer_exprs(attrs: &[syn::Attribute]) -> syn::Result<Vec<syn::Expr>> {
    let mut exprs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("layer") {
            let expr: syn::Expr = attr.parse_args()?;
            exprs.push(expr);
        }
    }
    Ok(exprs)
}

pub fn extract_scheduled(attrs: &[syn::Attribute]) -> syn::Result<Option<ScheduledConfig>> {
    for attr in attrs {
        if attr.path().is_ident("scheduled") {
            let mut every: Option<u64> = None;
            let mut cron: Option<String> = None;
            let mut initial_delay: Option<u64> = None;
            let mut name: Option<String> = None;

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("every") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    every = Some(lit.base10_parse()?);
                    Ok(())
                } else if meta.path.is_ident("cron") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    cron = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("initial_delay") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    initial_delay = Some(lit.base10_parse()?);
                    Ok(())
                } else if meta.path.is_ident("name") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    name = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `every`, `cron`, `initial_delay`, or `name`"))
                }
            })?;

            if every.is_none() && cron.is_none() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[scheduled] requires either `every` or `cron`",
                ));
            }
            if every.is_some() && cron.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[scheduled] cannot have both `every` and `cron`",
                ));
            }
            if initial_delay.is_some() && cron.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`initial_delay` is not compatible with `cron`",
                ));
            }

            return Ok(Some(ScheduledConfig {
                every,
                cron,
                initial_delay,
                name,
            }));
        }
    }
    Ok(None)
}

/// Extracts parameters marked with `#[managed]` and strips the attribute.
///
/// The parameter type must be `&mut T` where `T: ManagedResource<S>`.
/// Returns the list of managed parameters with their indices and inner types.
pub fn extract_managed_params(method: &mut syn::ImplItemFn) -> syn::Result<Vec<ManagedParam>> {
    let mut managed_params = Vec::new();
    let mut param_idx = 0usize;

    for arg in method.sig.inputs.iter_mut() {
        if let syn::FnArg::Typed(pat_type) = arg {
            let is_managed = pat_type.attrs.iter().any(|a| a.path().is_ident("managed"));

            if is_managed {
                // Validate that the type is &mut T
                let inner_ty = extract_mut_ref_inner(&pat_type.ty).ok_or_else(|| {
                    syn::Error::new(
                        pat_type.ty.span(),
                        "#[managed] parameters must be mutable references: `&mut Tx<...>`",
                    )
                })?;

                managed_params.push(ManagedParam {
                    index: param_idx,
                    ty: inner_ty,
                });

                // Strip the managed attribute
                pat_type.attrs.retain(|a| !a.path().is_ident("managed"));
            }
            param_idx += 1;
        }
    }
    Ok(managed_params)
}

/// Extracts the inner type from a `&mut T` reference type.
fn extract_mut_ref_inner(ty: &syn::Type) -> Option<syn::Type> {
    if let syn::Type::Reference(ref_ty) = ty {
        if ref_ty.mutability.is_some() {
            return Some((*ref_ty.elem).clone());
        }
    }
    None
}
