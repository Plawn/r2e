//! Shared `#[scheduled]` codegen helpers, used by both the controller path
//! (`controller_impl::generate_transverse`) and the bean path
//! (`bean_attr`'s `ScheduledSource` generation), via `codegen::transverse`.

use proc_macro2::TokenStream;
use quote::quote;

use crate::types::{OverlapMode, ScheduledConfig};

/// Generate the `ScheduleConfig` expression for a parsed `#[scheduled]` config.
pub(crate) fn schedule_config_expr(
    config: &ScheduledConfig,
    sched_krate: &TokenStream,
) -> TokenStream {
    if let Some(every_ms) = config.every_ms {
        if let Some(delay_ms) = config.initial_delay_ms {
            quote! {
                #sched_krate::ScheduleConfig::IntervalWithDelay {
                    interval: std::time::Duration::from_millis(#every_ms),
                    initial_delay: std::time::Duration::from_millis(#delay_ms),
                }
            }
        } else {
            quote! {
                #sched_krate::ScheduleConfig::Interval(
                    std::time::Duration::from_millis(#every_ms)
                )
            }
        }
    } else {
        let cron_expr = config.cron.as_ref().unwrap();
        quote! {
            #sched_krate::ScheduleConfig::Cron(#cron_expr.to_string())
        }
    }
}

/// Generate the `OverlapPolicy` expression for a parsed `#[scheduled]` config.
pub(crate) fn overlap_policy_expr(overlap: OverlapMode, sched_krate: &TokenStream) -> TokenStream {
    match overlap {
        OverlapMode::Skip => quote! { #sched_krate::OverlapPolicy::Skip },
        OverlapMode::Concurrent => quote! { #sched_krate::OverlapPolicy::Concurrent },
    }
}

/// Default task name for a `#[scheduled]` method: `<Owner>_<method>`, unless
/// an explicit `name = "..."` was given.
pub(crate) fn task_name(config: &ScheduledConfig, owner: &str, fn_name: &str) -> String {
    match &config.name {
        Some(n) => n.clone(),
        None => format!("{}_{}", owner, fn_name),
    }
}

/// A resolved `skip_if = "..."` predicate: the method's ident plus whether the
/// emitted call must be awaited.
pub(crate) struct SkipCall {
    pub fn_name: syn::Ident,
    pub is_async: bool,
}

/// Resolve a `#[scheduled(skip_if = "...")]` value against the host impl
/// block's *plain* methods (no route/transverse marker — each host filters
/// before calling). The predicate must be a `&self`-only method, sync or
/// async; its `bool` return is enforced by an ascription at the call site.
pub(crate) fn resolve_skip_if<'a>(
    config: &ScheduledConfig,
    mut plain_methods: impl Iterator<Item = &'a syn::ImplItemFn>,
) -> syn::Result<Option<SkipCall>> {
    let Some(lit) = &config.skip_if else {
        return Ok(None);
    };
    let target = lit.value();
    let Some(method) = plain_methods.find(|m| m.sig.ident == target) else {
        return Err(syn::Error::new(
            lit.span(),
            format!(
                "skip_if = \"{target}\" does not name a plain method in this impl block\n\n\
                 The predicate must be a plain `&self` method (sync or async) returning `bool`, \
                 defined in the same impl block as the #[scheduled] method — it cannot carry a \
                 route, #[scheduled], #[consumer], #[async_exec], or lifecycle marker.\n\n\
                 example:\n  fn maintenance_mode(&self) -> bool {{ /* ... */ }}\n\n  \
                 #[scheduled(every = \"5m\", skip_if = \"maintenance_mode\")]\n  \
                 async fn sync(&self) {{ /* ... */ }}\n\n\
                 To skip on a shared condition (Quarkus skipExecutionIf-style predicate bean), \
                 #[inject] the predicate bean and delegate to it from the method."
            ),
        ));
    };
    let has_self = method
        .sig
        .inputs
        .iter()
        .any(|arg| matches!(arg, syn::FnArg::Receiver(_)));
    if !has_self || method.sig.inputs.len() > 1 {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "a skip_if predicate must take only `&self` (no parameters) and return `bool`",
        ));
    }
    Ok(Some(SkipCall {
        fn_name: method.sig.ident.clone(),
        is_async: method.sig.asyncness.is_some(),
    }))
}
