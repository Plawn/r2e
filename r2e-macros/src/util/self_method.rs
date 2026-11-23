//! Resolution of `<key> = "method"` attribute values — a string naming a
//! plain `&self` method of the host impl block. Shared by
//! `#[scheduled(schedule = "..", skip_if = "..")]` and
//! `#[ws(max_message_size = "..", max_frame_size = "..")]`.

/// Resolve a `<key> = "method"` literal against the host impl block's *plain*
/// methods (no route/transverse marker — each host filters before calling).
/// The target must be a `&self`-only method; `what`/`example` flavour the
/// errors. Returns the method ident and whether it is `async`.
pub(crate) fn resolve_self_method<'a>(
    lit: &syn::LitStr,
    mut plain_methods: impl Iterator<Item = &'a syn::ImplItemFn>,
    not_found: impl FnOnce(&str) -> String,
    bad_signature: &str,
) -> syn::Result<(syn::Ident, bool)> {
    let target = lit.value();
    let Some(method) = plain_methods.find(|m| m.sig.ident == target) else {
        return Err(syn::Error::new(lit.span(), not_found(&target)));
    };
    let has_self = method
        .sig
        .inputs
        .iter()
        .any(|arg| matches!(arg, syn::FnArg::Receiver(_)));
    if !has_self || method.sig.inputs.len() > 1 {
        return Err(syn::Error::new_spanned(&method.sig, bad_signature));
    }
    Ok((method.sig.ident.clone(), method.sig.asyncness.is_some()))
}
