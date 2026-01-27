use crate::attr_extract::*;
use crate::types::*;

/// Parsed representation of a `#[routes] impl Name { ... }` block.
pub struct RoutesImplDef {
    pub controller_name: syn::Ident,
    pub controller_intercepts: Vec<syn::Expr>,
    pub route_methods: Vec<RouteMethod>,
    pub consumer_methods: Vec<ConsumerMethod>,
    pub scheduled_methods: Vec<ScheduledMethod>,
    pub other_methods: Vec<syn::ImplItemFn>,
}

pub fn parse(item: syn::ItemImpl) -> syn::Result<RoutesImplDef> {
    // Extract controller name from self type
    let controller_name = match *item.self_ty {
        syn::Type::Path(ref type_path) => type_path
            .path
            .segments
            .last()
            .ok_or_else(|| syn::Error::new_spanned(&item.self_ty, "expected type name"))?
            .ident
            .clone(),
        _ => {
            return Err(syn::Error::new_spanned(
                &item.self_ty,
                "expected a type path",
            ))
        }
    };

    // Extract controller-level intercepts from impl attrs
    let controller_intercepts = extract_intercept_fns(&item.attrs)?;

    // Classify methods
    let mut route_methods = Vec::new();
    let mut consumer_methods = Vec::new();
    let mut scheduled_methods = Vec::new();
    let mut other_methods = Vec::new();

    for impl_item in item.items {
        match impl_item {
            syn::ImplItem::Fn(mut method) => {
                let all_attrs = std::mem::take(&mut method.attrs);

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
                } else if let Some(config) = extract_scheduled(&all_attrs)? {
                    let intercept_fns = extract_intercept_fns(&all_attrs)?;
                    let has_extra_params = method
                        .sig
                        .inputs
                        .iter()
                        .any(|arg| matches!(arg, syn::FnArg::Typed(_)));
                    if has_extra_params {
                        return Err(syn::Error::new(
                            method.sig.ident.span(),
                            "scheduled methods cannot have parameters other than &self",
                        ));
                    }
                    method.attrs = strip_scheduled_attrs(all_attrs);
                    scheduled_methods.push(ScheduledMethod {
                        config,
                        intercept_fns,
                        fn_item: method,
                    });
                } else if let Some((http_method, path)) = extract_route_attr(&all_attrs)? {
                    let roles = extract_roles(&all_attrs)?;
                    let transactional = extract_transactional(&all_attrs)?;
                    let intercept_fns = extract_intercept_fns(&all_attrs)?;
                    let middleware_fns = extract_middleware_fns(&all_attrs)?;

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
            }
            _ => {} // skip non-method items
        }
    }

    Ok(RoutesImplDef {
        controller_name,
        controller_intercepts,
        route_methods,
        consumer_methods,
        scheduled_methods,
        other_methods,
    })
}
