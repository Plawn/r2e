//! Scheduled task attribute extraction.

use crate::types::ScheduledConfig;

pub fn strip_scheduled_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("scheduled") && !a.path().is_ident("intercept"))
        .collect()
}

/// Parse a duration string like "5s", "2m", "1h30m", "500ms", "1m30s" into milliseconds.
///
/// Supported suffixes: `ms`, `s`, `m`, `h`, `d`.
/// Multiple segments can be combined: `"1h30m"`, `"2m30s"`, `"1h15m30s"`.
fn parse_duration_ms(input: &str) -> Result<u64, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty duration string".to_string());
    }

    let mut total_ms: u64 = 0;
    let mut current_num = String::new();
    let mut chars = s.chars().peekable();
    let mut found_any = false;

    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_digit() {
            current_num.push(ch);
            chars.next();
        } else if ch.is_ascii_alphabetic() {
            if current_num.is_empty() {
                return Err(format!("unexpected '{}' without a preceding number", ch));
            }

            let mut suffix = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_alphabetic() {
                    suffix.push(c);
                    chars.next();
                } else {
                    break;
                }
            }

            let num: u64 = current_num
                .parse()
                .map_err(|_| format!("invalid number: '{}'", current_num))?;

            let multiplier = match suffix.as_str() {
                "ms" => 1,
                "s" => 1_000,
                "m" => 60_000,
                "h" => 3_600_000,
                "d" => 86_400_000,
                _ => return Err(format!("unknown duration suffix '{}' — use ms, s, m, h, or d", suffix)),
            };

            total_ms += num * multiplier;
            current_num.clear();
            found_any = true;
        } else {
            return Err(format!("unexpected character '{}' in duration", ch));
        }
    }

    if !current_num.is_empty() {
        return Err(format!(
            "trailing number '{}' without a unit suffix (ms, s, m, h, d)",
            current_num
        ));
    }

    if !found_any {
        return Err("no duration segments found".to_string());
    }

    if total_ms == 0 {
        return Err("duration must be greater than zero".to_string());
    }

    Ok(total_ms)
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
            return Err(syn::Error::new(lit.span(), "duration must be greater than zero"));
        }
        Ok(secs * 1_000) // convert seconds to ms
    } else if lookahead.peek(syn::LitStr) {
        let lit: syn::LitStr = value.parse()?;
        parse_duration_ms(&lit.value())
            .map_err(|e| syn::Error::new(lit.span(), format!("invalid duration '{}': {}", lit.value(), e)))
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
                } else {
                    Err(meta.error(
                        "unknown key in #[scheduled(...)]: expected `every`, `cron`, `initial_delay`, or `name`\n\n\
                         examples:\n  #[scheduled(every = 30)]\n  #[scheduled(every = \"5m\")]\n  \
                         #[scheduled(cron = \"0 */5 * * * *\")]\n  \
                         #[scheduled(every = \"1h\", initial_delay = \"10s\")]"
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
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_seconds() {
        assert_eq!(parse_duration_ms("30s").unwrap(), 30_000);
    }

    #[test]
    fn parse_minutes() {
        assert_eq!(parse_duration_ms("5m").unwrap(), 300_000);
    }

    #[test]
    fn parse_hours() {
        assert_eq!(parse_duration_ms("2h").unwrap(), 7_200_000);
    }

    #[test]
    fn parse_days() {
        assert_eq!(parse_duration_ms("1d").unwrap(), 86_400_000);
    }

    #[test]
    fn parse_millis() {
        assert_eq!(parse_duration_ms("500ms").unwrap(), 500);
    }

    #[test]
    fn parse_combined() {
        assert_eq!(parse_duration_ms("1h30m").unwrap(), 5_400_000);
        assert_eq!(parse_duration_ms("2m30s").unwrap(), 150_000);
        assert_eq!(parse_duration_ms("1h15m30s").unwrap(), 4_530_000);
    }

    #[test]
    fn parse_zero_rejected() {
        assert!(parse_duration_ms("0s").is_err());
    }

    #[test]
    fn parse_no_unit_rejected() {
        assert!(parse_duration_ms("30").is_err());
    }

    #[test]
    fn parse_empty_rejected() {
        assert!(parse_duration_ms("").is_err());
    }

    #[test]
    fn parse_unknown_suffix_rejected() {
        assert!(parse_duration_ms("5x").is_err());
    }
}
