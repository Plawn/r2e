//! Scheduled task attribute extraction.

use crate::types::ScheduledConfig;

pub fn strip_scheduled_attrs(attrs: Vec<syn::Attribute>) -> Vec<syn::Attribute> {
    attrs
        .into_iter()
        .filter(|a| !a.path().is_ident("scheduled") && !a.path().is_ident("intercept"))
        .collect()
}

pub fn extract_scheduled(attrs: &[syn::Attribute]) -> syn::Result<Option<ScheduledConfig>> {
    for attr in attrs {
        if attr.path().is_ident("scheduled") {
            let mut every: Option<u64> = None;
            let mut cron: Option<String> = None;
            let mut initial_delay: Option<u64> = None;
            let mut name: Option<String> = None;

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("every") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    every = Some(lit.base10_parse()?);
                    Ok(())
                } else if meta.path.is_ident("cron") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    cron = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("initial_delay") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    initial_delay = Some(lit.base10_parse()?);
                    Ok(())
                } else if meta.path.is_ident("name") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    name = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `every`, `cron`, `initial_delay`, or `name`"))
                }
            })?;

            if every.is_none() && cron.is_none() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[scheduled] requires either `every` or `cron`",
                ));
            }
            if every.is_some() && cron.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "#[scheduled] cannot have both `every` and `cron`",
                ));
            }
            if initial_delay.is_some() && cron.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`initial_delay` is not compatible with `cron`",
                ));
            }

            return Ok(Some(ScheduledConfig {
                every,
                cron,
                initial_delay,
                name,
            }));
        }
    }
    Ok(None)
}
