//! Scheduled task attribute extraction.

use super::duration::parse_duration_ms;
use crate::types::{OverlapMode, ScheduledConfig};

pub fn strip_scheduled_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("scheduled") && !a.path().is_ident("intercept"))
        .collect()
}

/// Parse a duration value from either an integer literal (seconds) or a string literal ("5m", "2h").
/// Returns milliseconds.
fn parse_duration_value(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<u64> {
    let value = meta.value()?;
    let lookahead = value.lookahead1();

    if lookahead.peek(syn::LitInt) {
        let lit: syn::LitInt = value.parse()?;
        let secs: u64 = lit.base10_parse()?;
        if secs == 0 {
            return Err(syn::Error::new(
                lit.span(),
                "duration must be greater than zero",
            ));
        }
        Ok(secs * 1_000) // convert seconds to ms
    } else if lookahead.peek(syn::LitStr) {
        let lit: syn::LitStr = value.parse()?;
        parse_duration_ms(&lit.value()).map_err(|e| {
            syn::Error::new(
                lit.span(),
                format!("invalid duration '{}': {}", lit.value(), e),
            )
        })
    } else {
        Err(lookahead.error())
    }
}

pub fn extract_scheduled(attrs: &[syn::Attribute]) -> syn::Result<Option<ScheduledConfig>> {
    for attr in attrs {
        if attr.path().is_ident("scheduled") {
            let mut every_ms: Option<u64> = None;
            let mut cron: Option<String> = None;
            let mut initial_delay_ms: Option<u64> = None;
            let mut name: Option<String> = None;
            let mut overlap: OverlapMode = OverlapMode::Skip;
            let mut skip_if: Option<syn::LitStr> = None;

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("every") {
                    every_ms = Some(parse_duration_value(&meta)?);
                    Ok(())
                } else if meta.path.is_ident("cron") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    let expr = lit.value();

                    // Validate cron expression at compile time
                    if let Err(e) = expr.parse::<cron::Schedule>() {
                        return Err(syn::Error::new(
                            lit.span(),
                            format!("invalid cron expression '{}': {}", expr, e),
                        ));
                    }

                    cron = Some(expr);
                    Ok(())
                } else if meta.path.is_ident("initial_delay") {
                    initial_delay_ms = Some(parse_duration_value(&meta)?);
                    Ok(())
                } else if meta.path.is_ident("name") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    name = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("overlap") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    overlap = match lit.value().as_str() {
                        "skip" => OverlapMode::Skip,
                        "concurrent" => OverlapMode::Concurrent,
                        other => {
                            return Err(syn::Error::new(
                                lit.span(),
                                format!(
                                    "invalid overlap policy '{}': expected \"skip\" or \"concurrent\"",
                                    other
                                ),
                            ))
                        }
                    };
                    Ok(())
                } else if meta.path.is_ident("skip_if") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    if lit.value().is_empty() {
                        return Err(syn::Error::new(
                            lit.span(),
                            "`skip_if` must name a method on the same impl block, e.g. skip_if = \"maintenance_mode\"",
                        ));
                    }
                    skip_if = Some(lit);
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown key in #[scheduled(...)]: expected `every`, `cron`, `initial_delay`, `name`, `overlap`, or `skip_if`\n\n\
                         examples:\n  #[scheduled(every = 30)]\n  #[scheduled(every = \"5m\")]\n  \
                         #[scheduled(cron = \"0 */5 * * * *\")]\n  \
                         #[scheduled(every = \"1h\", initial_delay = \"10s\")]\n  \
                         #[scheduled(every = \"50ms\", overlap = \"concurrent\")]\n  \
                         #[scheduled(every = \"5m\", skip_if = \"maintenance_mode\")]"
                    ))
                }
            })?;

            if every_ms.is_none() && cron.is_none() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[scheduled] requires either `every` (interval) or `cron` (expression):\n\
                     \n  #[scheduled(every = 30)]                    — run every 30 seconds\n\
                     \n  #[scheduled(every = \"5m\")]                  — run every 5 minutes\n\
                     \n  #[scheduled(cron = \"0 */5 * * * *\")]        — cron schedule",
                ));
            }
            if every_ms.is_some() && cron.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`every` and `cron` are mutually exclusive — use one or the other",
                ));
            }
            if initial_delay_ms.is_some() && cron.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`initial_delay` only works with `every` (interval-based schedules), not with `cron`\n\n\
                     For cron, the schedule itself controls the first execution time.",
                ));
            }

            return Ok(Some(ScheduledConfig {
                every_ms,
                cron,
                initial_delay_ms,
                name,
                overlap,
                skip_if,
            }));
        }
    }
    Ok(None)
}
