//! Managed resource attribute extraction.

use crate::types::ManagedParam;
use syn::spanned::Spanned;

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
                        "#[managed] parameter must be a mutable reference (`&mut T`):\n\
                         \n  #[managed] tx: &mut Tx<'_, Sqlite>\n\n\
                         The resource is acquired before the handler and released after it.",
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
