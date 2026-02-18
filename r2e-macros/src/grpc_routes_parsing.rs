//! Parsing for the `#[grpc_routes(TraitPath)]` attribute macro.

use crate::derive_parsing::has_identity_qualifier;
use crate::extract::*;
use crate::types::{IdentityParam, MethodDecorators};

/// Parsed representation of a `#[grpc_routes(TraitPath)] impl Name { ... }` block.
pub struct GrpcRoutesImplDef {
    /// The controller struct name (e.g., `UserGrpcService`).
    pub controller_name: syn::Ident,
    /// The tonic-generated service trait path (e.g., `proto::user_service_server::UserService`).
    pub service_trait: syn::Path,
    /// Controller-level interceptors applied to all methods.
    pub controller_intercepts: Vec<syn::Expr>,
    /// gRPC methods with their attributes.
    pub methods: Vec<GrpcMethod>,
    /// Non-gRPC methods (helper functions, etc.) passed through unchanged.
    pub other_methods: Vec<syn::ImplItemFn>,
}

/// A single gRPC method with parsed attributes.
pub struct GrpcMethod {
    /// Method name (must match the tonic trait method name).
    pub name: syn::Ident,
    /// Parsed decorator attributes (gRPC currently supports interceptors only).
    pub decorators: MethodDecorators,
    /// Identity parameter (if `#[inject(identity)]` is on a handler param).
    pub identity_param: Option<IdentityParam>,
    /// The original method item (with route attrs stripped).
    pub fn_item: syn::ImplItemFn,
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

/// Detect `#[inject(identity)]` on handler parameters.
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
                        "only one #[inject(identity)] parameter is allowed per gRPC handler",
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

/// Parse a `#[grpc_routes(TraitPath)] impl Name { ... }` block.
pub fn parse(
    service_trait: syn::Path,
    item: syn::ItemImpl,
) -> syn::Result<GrpcRoutesImplDef> {
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

    let mut methods = Vec::new();
    let mut other_methods = Vec::new();

    for impl_item in item.items {
        match impl_item {
            syn::ImplItem::Fn(mut method) => {
                let all_attrs = std::mem::take(&mut method.attrs);

                // Every async method in the impl block is considered a gRPC method
                // (matching the tonic trait). Non-async or methods without &self are helpers.
                let is_receiver = method
                    .sig
                    .inputs
                    .first()
                    .map_or(false, |arg| matches!(arg, syn::FnArg::Receiver(_)));

                if method.sig.asyncness.is_some() && is_receiver {
                    let decorators = parse_grpc_decorators(&all_attrs)?;

                    method.attrs = strip_known_attrs(all_attrs);
                    let identity_param = extract_identity_param(&mut method)?;
                    let name = method.sig.ident.clone();

                    methods.push(GrpcMethod {
                        name,
                        decorators,
                        identity_param,
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

    Ok(GrpcRoutesImplDef {
        controller_name,
        service_trait,
        controller_intercepts,
        methods,
        other_methods,
    })
}
