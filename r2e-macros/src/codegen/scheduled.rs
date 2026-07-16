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
pub(crate) fn overlap_policy_expr(
    overlap: OverlapMode,
    sched_krate: &TokenStream,
) -> TokenStream {
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
