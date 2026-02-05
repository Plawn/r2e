use crate::extract::*;
use crate::derive_parsing::has_identity_qualifier;
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

/// Try to unwrap `Option<T>` → `Some(T)`, or `None` if not an Option.
fn unwrap_option_type(ty: &syn::Type) -> Option<&syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(ref args) = segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Detect `#[inject(identity)]` or legacy `#[identity]` on handler parameters.
/// Returns the parameter index (among typed params, excluding `&self`) and the
/// parameter type if found. Strips the attribute from the parameter.
fn extract_identity_param(method: &mut syn::ImplItemFn) -> syn::Result<Option<IdentityParam>> {
    let mut identity_param = None;
    let mut param_idx = 0usize;

    for arg in method.sig.inputs.iter_mut() {
        if let syn::FnArg::Typed(pat_type) = arg {
            let is_identity = pat_type.attrs.iter().any(|a| {
                (a.path().is_ident("inject") && has_identity_qualifier(a))
                    || a.path().is_ident("identity")
            });

            if is_identity {
                if identity_param.is_some() {
                    return Err(syn::Error::new_spanned(
                        pat_type,
                        "only one #[inject(identity)] parameter is allowed per handler",
                    ));
                }
                let declared_ty = (*pat_type.ty).clone();
                let (inner_ty, is_optional) = match unwrap_option_type(&declared_ty) {
                    Some(inner) => (inner.clone(), true),
                    None => (declared_ty, false),
                };
                identity_param = Some(IdentityParam {
                    index: param_idx,
                    ty: inner_ty,
                    is_optional,
                });
                // Strip the identity attribute
                pat_type.attrs.retain(|a| {
                    !((a.path().is_ident("inject") && has_identity_qualifier(a))
                        || a.path().is_ident("identity"))
                });
            }
            param_idx += 1;
        }
    }
    Ok(identity_param)
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
                    let layer_exprs = extract_layer_exprs(&all_attrs)?;

                    let mut guard_fns = Vec::new();
                    let mut pre_auth_guard_fns = Vec::new();

                    let rl_guards = extract_rate_limited_guards(&all_attrs)?;
                    if let Some(post_auth) = rl_guards.post_auth {
                        guard_fns.push(post_auth);
                    }
                    if let Some(pre_auth) = rl_guards.pre_auth {
                        pre_auth_guard_fns.push(pre_auth);
                    }

                    if let Some(roles_guard) = roles_guard_expr(&roles) {
                        guard_fns.push(roles_guard);
                    }
                    guard_fns.extend(extract_guard_fns(&all_attrs)?);
                    pre_auth_guard_fns.extend(extract_pre_guard_fns(&all_attrs)?);

                    method.attrs = strip_route_attrs(all_attrs);

                    // Detect #[inject(identity)] on handler params
                    let identity_param = extract_identity_param(&mut method)?;

                    // Detect #[managed] on handler params
                    let managed_params = extract_managed_params(&mut method)?;

                    // Validate: #[managed] and #[transactional] are mutually exclusive
                    if transactional.is_some() && !managed_params.is_empty() {
                        return Err(syn::Error::new(
                            method.sig.ident.span(),
                            "#[managed] and #[transactional] cannot be used together; \
                             use #[managed] tx: &mut Tx<...> instead of #[transactional]",
                        ));
                    }

                    route_methods.push(RouteMethod {
                        method: http_method,
                        path,
                        roles,
                        transactional,
                        intercept_fns,
                        guard_fns,
                        pre_auth_guard_fns,
                        middleware_fns,
                        layer_exprs,
                        identity_param,
                        managed_params,
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
