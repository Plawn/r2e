//! Shared `#[scheduled]` codegen helpers, used by both the controller path
//! (`controller_impl::generate_transverse`) and the bean path
//! (`bean_attr`'s `ScheduledSource` generation), via `codegen::transverse`.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};

use crate::model::types::{OverlapMode, ScheduledConfig};

/// Generate the `ScheduleConfig` expression for a parsed `#[scheduled]` config.
///
/// `instance` is the expression the emission site has in scope (`__core` on the
/// controller path, `self` on the bean path) and `schedule` the resolved
/// `schedule = "method"` ident, if any: the cadence is then read **once, at
/// registration**, by calling that method synchronously on the instance. The
/// `ScheduleConfig` ascription pins the method's return type so a mistyped
/// method fails with a readable rustc error (same trick as the `bool`
/// ascription on `skip_if`).
pub(crate) fn schedule_config_expr(
    config: &ScheduledConfig,
    sched_krate: &TokenStream,
    instance: &TokenStream,
    schedule: Option<&syn::Ident>,
) -> TokenStream {
    if let Some(method) = schedule {
        // The ident carries the `schedule = "..."` literal's span (see
        // `resolve_schedule`). rustc anchors a method-call type mismatch on
        // the receiver's span, so the receiver tokens (`__core` / `self`,
        // minted at the host macro's call site) are re-spanned too — the
        // error then points at the attribute rather than at `#[routes]`.
        let span = method.span();
        let respan = |ts: &TokenStream| -> TokenStream {
            ts.clone()
                .into_iter()
                .map(|mut tt| {
                    tt.set_span(span);
                    tt
                })
                .collect()
        };
        let instance = respan(instance);
        let sched_krate = respan(sched_krate);
        return quote_spanned! { span =>
            {
                let __schedule: #sched_krate::ScheduleConfig = #instance.#method();
                __schedule
            }
        };
    }
    if let Some(every_ms) = config.every_ms {
        // Construct the positive interval in a `const` block: a zero interval
        // (`every = "0s"` / `every = 0`) fails const-eval → a compile error,
        // so the runtime type only ever sees a strictly-positive interval.
        let interval = quote! {
            const {
                match #sched_krate::PositiveDuration::from_millis(#every_ms) {
                    Some(d) => d,
                    None => panic!("#[scheduled] interval must be greater than zero"),
                }
            }
        };
        if let Some(delay_ms) = config.initial_delay_ms {
            quote! {
                #sched_krate::ScheduleConfig::IntervalWithDelay {
                    interval: #interval,
                    initial_delay: std::time::Duration::from_millis(#delay_ms),
                }
            }
        } else {
            quote! {
                #sched_krate::ScheduleConfig::Interval(#interval)
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

/// Resolve a `<key> = "method"` literal against the host impl block's *plain*
/// methods (no route/transverse marker — each host filters before calling).
/// The target must be a `&self`-only method; `what`/`example` flavour the
/// errors. Returns the method ident and whether it is `async`.
fn resolve_self_method<'a>(
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

/// Resolve a `#[scheduled(skip_if = "...")]` value against the host impl
/// block's *plain* methods (no route/transverse marker — each host filters
/// before calling). The predicate must be a `&self`-only method, sync or
/// async; its `bool` return is enforced by an ascription at the call site.
pub(crate) fn resolve_skip_if<'a>(
    config: &ScheduledConfig,
    plain_methods: impl Iterator<Item = &'a syn::ImplItemFn>,
) -> syn::Result<Option<SkipCall>> {
    let Some(lit) = &config.skip_if else {
        return Ok(None);
    };
    let (fn_name, is_async) = resolve_self_method(
        lit,
        plain_methods,
        |target| {
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
            )
        },
        "a skip_if predicate must take only `&self` (no parameters) and return `bool`",
    )?;
    Ok(Some(SkipCall { fn_name, is_async }))
}

/// Resolve a `#[scheduled(schedule = "...")]` value against the host impl
/// block's *plain* methods. The cadence provider must be a **sync** `&self`-only
/// method: it is called once at registration, where the `ScheduledTaskDef` is
/// built (a sync context on both the controller and the bean path). Its
/// `ScheduleConfig` return is enforced by an ascription at the call site.
pub(crate) fn resolve_schedule<'a>(
    config: &ScheduledConfig,
    plain_methods: impl Iterator<Item = &'a syn::ImplItemFn>,
) -> syn::Result<Option<syn::Ident>> {
    let Some(lit) = &config.schedule else {
        return Ok(None);
    };
    let (fn_name, is_async) = resolve_self_method(
        lit,
        plain_methods,
        |target| {
            format!(
                "schedule = \"{target}\" does not name a plain method in this impl block\n\n\
                 The cadence provider must be a plain, sync `&self` method returning \
                 `ScheduleConfig`, defined in the same impl block as the #[scheduled] method — \
                 it cannot carry a route, #[scheduled], #[consumer], #[async_exec], or lifecycle \
                 marker.\n\n\
                 example:\n  fn sync_cadence(&self) -> ScheduleConfig {{ self.cadence.clone() }}\n\n  \
                 #[scheduled(schedule = \"sync_cadence\")]\n  \
                 async fn sync(&self) {{ /* ... */ }}\n\n\
                 The method is called once at registration; read the cadence from a \
                 `#[config(\"...\")] cadence: ScheduleConfig` field, an injected bean, or compute it."
            )
        },
        "a schedule provider must take only `&self` (no parameters) and return `ScheduleConfig`",
    )?;
    if is_async {
        return Err(syn::Error::new(
            lit.span(),
            format!(
                "schedule = \"{}\" names an `async fn` — the cadence provider must be sync\n\n\
                 The cadence is resolved once at registration, synchronously, when the task \
                 definition is built. Make the method a plain `fn(&self) -> ScheduleConfig`.",
                lit.value()
            ),
        ));
    }
    // Re-span the ident on the attribute literal so the `ScheduleConfig`
    // ascription at the call site reports a wrong return type there.
    Ok(Some(syn::Ident::new(&fn_name.to_string(), lit.span())))
}
