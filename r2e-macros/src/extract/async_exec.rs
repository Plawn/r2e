//! `#[async_exec]` attribute extraction.
//!
//! Marks a method on a `#[routes]` impl block to run on the injected
//! `PoolExecutor`. The original body is renamed to
//! `__r2e_async_<name>_inner` and the public method becomes a wrapper that
//! submits the body to the pool and returns a `JobHandle<T>`.

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
