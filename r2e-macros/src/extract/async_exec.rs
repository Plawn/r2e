//! `#[async_exec]` attribute extraction.
//!
//! Marks a method on a `#[routes]` impl block to run on the injected
//! `PoolExecutor`. The original body is renamed to
//! `__r2e_async_<name>_inner` and the public method becomes a wrapper that
//! submits the body to the pool and returns a `Result<JobHandle<T>, RejectedError>`.

use syn::spanned::Spanned;

/// Configuration parsed from `#[async_exec(...)]`.
#[derive(Debug, Clone)]
pub struct AsyncExecConfig {
    /// Name of the controller field holding the `PoolExecutor`.
    /// Defaults to `executor`.
    pub executor_field: syn::Ident,
}

pub fn strip_async_exec_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("async_exec"))
        .collect()
}

/// Host of an `#[async_exec]` method: a plain `#[bean]` impl or a `#[routes]`
/// controller impl. Selects the conflict matrix (controllers add the request
/// wiring markers — route verbs, `#[sse]`, `#[ws]`, `#[fallback]`) and the
/// receiver-error wording (a bean points at a `PoolExecutor` field; a
/// controller at an `#[inject] PoolExecutor` field).
#[derive(Clone, Copy)]
pub enum AsyncExecHost {
    Bean,
    Controller,
}

/// Single source of truth for the "requires async fn" diagnostic (shared by
/// bean and controller hosts).
pub(crate) const ASYNC_EXEC_ASYNC_MSG: &str =
    "#[async_exec] requires an `async fn` — the body is submitted to a PoolExecutor";

/// Single source of truth for the "intercept not supported" diagnostic (shared
/// by bean and controller hosts).
pub(crate) const ASYNC_EXEC_INTERCEPT_MSG: &str =
    "#[intercept] on an #[async_exec] method is not supported — the \
     pool-submission wrapper does not run an interceptor chain";

impl AsyncExecHost {
    /// Owner-specific error text for a missing `&self` receiver.
    fn no_receiver_msg(self) -> &'static str {
        match self {
            AsyncExecHost::Bean => {
                "#[async_exec] methods must take `&self` as the first argument. \
                 The bean also needs a `PoolExecutor` field \
                 (default name: `executor`; override with #[async_exec(executor = \"name\")])"
            }
            AsyncExecHost::Controller => {
                "#[async_exec] methods must take `&self` as the first argument. \
                 The controller also needs an `#[inject] PoolExecutor` field \
                 (default name: `executor`; override with `#[async_exec(executor = \"name\")]`)"
            }
        }
    }

    /// A marker on `attrs` that cannot co-exist with `#[async_exec]`, formatted
    /// for interpolation into the conflict diagnostic. Both hosts reject
    /// `#[scheduled]`/`#[consumer]`/`#[post_construct]`; controllers also reject
    /// the request-wiring markers, which would otherwise be classified first
    /// and silently swallow the `#[async_exec]` rewrite (route → 404,
    /// scheduled/consumer → retained no-op attr).
    fn conflicting_marker(self, attrs: &[syn::Attribute]) -> Option<&'static str> {
        if attrs.iter().any(|a| a.path().is_ident("scheduled")) {
            return Some("#[scheduled]");
        }
        if attrs.iter().any(|a| a.path().is_ident("consumer")) {
            return Some("#[consumer]");
        }
        if attrs.iter().any(|a| a.path().is_ident("post_construct")) {
            return Some("#[post_construct]");
        }
        if matches!(self, AsyncExecHost::Controller) {
            if attrs.iter().any(super::route::is_route_attr) {
                return Some("a route verb (#[get], #[post], ...)");
            }
            if attrs.iter().any(super::route::is_fallback_attr) {
                return Some("#[fallback]");
            }
            if attrs.iter().any(super::route::is_sse_attr) {
                return Some("#[sse]");
            }
            if attrs.iter().any(super::route::is_ws_attr) {
                return Some("#[ws]");
            }
        }
        None
    }
}

/// Full validation of an `#[async_exec]` method, shared by the bean and
/// controller hosts. Runs, in order:
///   1. the attr-conflict matrix (host-parameterized),
///   2. the `#[intercept]` rejection (no dispatch wrapper runs the chain),
///   3. the `async fn` requirement,
///   4. the `&self` receiver check.
///
/// On the controller path this MUST be called BEFORE the consumer/scheduled/
/// route classification branches, so a co-present conflicting marker is
/// rejected instead of swallowing the method.
pub fn validate_async_exec_method(
    attrs: &[syn::Attribute],
    sig: &syn::Signature,
    host: AsyncExecHost,
) -> syn::Result<()> {
    if let Some(label) = host.conflicting_marker(attrs) {
        return Err(syn::Error::new_spanned(
            sig,
            format!(
                "#[async_exec] cannot be combined with {label} on the same method — \
                 it rewrites the method into a pool-submission wrapper"
            ),
        ));
    }
    if attrs.iter().any(|a| a.path().is_ident("intercept")) {
        return Err(syn::Error::new(sig.ident.span(), ASYNC_EXEC_INTERCEPT_MSG));
    }
    if sig.asyncness.is_none() {
        return Err(syn::Error::new(sig.ident.span(), ASYNC_EXEC_ASYNC_MSG));
    }
    validate_async_exec_receiver(sig, host.no_receiver_msg())
}

/// Validate the receiver of an `#[async_exec]` method: it must be exactly
/// `&self`. The generated wrapper takes `&self` and submits a cloned handle
/// to the pool, so `&mut self` / by-value `self` cannot be forwarded.
///
/// `no_receiver_msg` is the owner-specific error for a missing receiver
/// (bean vs controller wording).
pub fn validate_async_exec_receiver(
    sig: &syn::Signature,
    no_receiver_msg: &str,
) -> syn::Result<()> {
    let receiver = sig.inputs.iter().find_map(|arg| match arg {
        syn::FnArg::Receiver(r) => Some(r),
        _ => None,
    });
    match receiver {
        None => Err(syn::Error::new(sig.ident.span(), no_receiver_msg)),
        Some(r) if r.reference.is_none() || r.mutability.is_some() => {
            Err(syn::Error::new_spanned(
                r,
                "#[async_exec] methods must take `&self` (not `&mut self` or `self`) — \
                 the generated wrapper clones an immutable handle to submit the body to the pool",
            ))
        }
        Some(_) => Ok(()),
    }
}

pub fn extract_async_exec(attrs: &[syn::Attribute]) -> syn::Result<Option<AsyncExecConfig>> {
    for attr in attrs {
        if attr.path().is_ident("async_exec") {
            let mut executor_field: Option<syn::Ident> = None;

            if matches!(attr.meta, syn::Meta::List(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("executor") {
                        let value = meta.value()?;
                        let lit: syn::LitStr = value.parse()?;
                        executor_field = Some(syn::Ident::new(&lit.value(), lit.span()));
                        Ok(())
                    } else {
                        Err(meta.error(
                            "unknown key in #[async_exec(...)]: expected `executor`\n\n\
                             example: #[async_exec(executor = \"my_pool\")]",
                        ))
                    }
                })?;
            }

            return Ok(Some(AsyncExecConfig {
                executor_field: executor_field
                    .unwrap_or_else(|| syn::Ident::new("executor", attr.span())),
            }));
        }
    }
    Ok(None)
}
