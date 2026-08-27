//! Shared parsing for transport method `#[inject(identity)]` parameters.

use crate::model::types::IdentityParam;
use crate::parsing::controller_parsing::has_identity_qualifier;
use crate::util::type_utils::unwrap_option_type;

/// Parse an identity marker and normalize its optional type. The legacy flag
/// preserves each adapter's existing `#[identity]` policy.
pub(crate) fn parse_identity_param(
    param: &syn::PatType,
    index: usize,
    allow_legacy_identity: bool,
) -> syn::Result<Option<IdentityParam>> {
    let legacy_attr = param.attrs.iter().find(|a| a.path().is_ident("identity"));
    if let Some(attr) = legacy_attr {
        if !allow_legacy_identity {
            return Err(syn::Error::new_spanned(
                attr,
                "`#[identity]` was removed; use `#[inject(identity)]`",
            ));
        }
    }

    let has_inject_identity = param
        .attrs
        .iter()
        .any(|a| a.path().is_ident("inject") && has_identity_qualifier(a));
    if !has_inject_identity && legacy_attr.is_none() {
        return Ok(None);
    }

    let declared_ty = (*param.ty).clone();
    let (ty, is_optional) = match unwrap_option_type(&declared_ty) {
        Some(inner) => (inner.clone(), true),
        None => (declared_ty, false),
    };

    Ok(Some(IdentityParam {
        index,
        ty,
        is_optional,
    }))
}

/// Strip the marker after duplicate checks, preserving their attributed span.
pub(crate) fn strip_identity_param_attrs(param: &mut syn::PatType, allow_legacy_identity: bool) {
    param.attrs.retain(|a| {
        !(a.path().is_ident("inject") && has_identity_qualifier(a)
            || allow_legacy_identity && a.path().is_ident("identity"))
    });
}
