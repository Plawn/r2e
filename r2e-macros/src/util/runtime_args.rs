//! Shared attribute-parsing helpers for the runtime-driving macros
//! (`#[r2e::main]` / `#[r2e::test]` and `#[r2e::test_suite]`).
//!
//! Both macros accept the same runtime-builder knobs and the same
//! literal-parsing helpers; [`RuntimeArgs`] owns that shared surface so the two
//! argument parsers only spell out their macro-specific keys.
//!
//! The emitted chain goes through `#krate::rt::RuntimeBuilder` — the facade —
//! rather than the runtime crate's own builder: user crates depend on `r2e` (or
//! `r2e_core`), never on `r2e-rt` directly, so the path has to be resolved
//! through [`r2e_core_path`](crate::util::crate_path::r2e_core_path) by the
//! caller and passed in as `krate`.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

/// The `rt::RuntimeBuilder` knobs shared by every runtime-driving macro.
///
/// `flavor`: `None` = user did not set it, `Some(true)` = current_thread,
/// `Some(false)` = multi_thread (the default).
#[derive(Default)]
pub(crate) struct RuntimeArgs {
    pub flavor: Option<bool>,
    pub worker_threads: Option<usize>,
    pub max_blocking_threads: Option<usize>,
    pub thread_stack_size: Option<usize>,
    pub thread_name: Option<String>,
    pub global_queue_interval: Option<u32>,
    pub event_interval: Option<u32>,
    pub thread_keep_alive_secs: Option<u64>,
    pub start_paused: Option<bool>,
}

impl RuntimeArgs {
    /// Try to consume `key` as one of the shared runtime knobs. Returns
    /// `Ok(true)` when the key was recognized and applied, `Ok(false)` when it
    /// is not a runtime knob (so the caller can match its own keys), and `Err`
    /// on a malformed value.
    pub fn try_parse(&mut self, key: &str, meta: &syn::meta::ParseNestedMeta) -> syn::Result<bool> {
        match key {
            "flavor" => {
                let s: syn::LitStr = meta.value()?.parse()?;
                self.flavor = Some(match s.value().as_str() {
                    "current_thread" => true,
                    "multi_thread" => false,
                    other => {
                        return Err(syn::Error::new_spanned(
                            &s,
                            format!(
                                "unknown flavor \"{other}\", expected \"current_thread\" or \"multi_thread\""
                            ),
                        ));
                    }
                });
            }
            "worker_threads" => self.worker_threads = Some(parse_int(meta)?),
            "max_blocking_threads" => self.max_blocking_threads = Some(parse_int(meta)?),
            "thread_stack_size" => self.thread_stack_size = Some(parse_int(meta)?),
            "thread_name" => self.thread_name = Some(parse_str(meta)?),
            "global_queue_interval" => self.global_queue_interval = Some(parse_int(meta)?),
            "event_interval" => self.event_interval = Some(parse_int(meta)?),
            "thread_keep_alive" => self.thread_keep_alive_secs = Some(parse_int(meta)?),
            "start_paused" => self.start_paused = Some(parse_bool(meta)?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Generate the full `rt::RuntimeBuilder` chain including `.build()`.
    /// Defaults to `new_multi_thread()` unless `flavor = "current_thread"`.
    ///
    /// `krate` is the resolved r2e-core root (`::r2e`, `::r2e_core` or
    /// `crate`); the builder is reached through its `rt` re-export.
    pub fn builder_tokens(&self, krate: &TokenStream2) -> TokenStream2 {
        let builder_fn = if self.flavor.unwrap_or(false) {
            quote! { #krate::rt::RuntimeBuilder::new_current_thread() }
        } else {
            quote! { #krate::rt::RuntimeBuilder::new_multi_thread() }
        };
        let worker_threads = self.worker_threads.map(|n| quote! { .worker_threads(#n) });
        let max_blocking = self
            .max_blocking_threads
            .map(|n| quote! { .max_blocking_threads(#n) });
        let stack_size = self
            .thread_stack_size
            .map(|n| quote! { .thread_stack_size(#n) });
        let thread_name = self
            .thread_name
            .as_ref()
            .map(|s| quote! { .thread_name(#s) });
        let gqi = self
            .global_queue_interval
            .map(|n| quote! { .global_queue_interval(#n) });
        let ei = self.event_interval.map(|n| quote! { .event_interval(#n) });
        let keep_alive = self.thread_keep_alive_secs.map(|secs| {
            quote! { .thread_keep_alive(::std::time::Duration::from_secs(#secs)) }
        });
        let start_paused = self.start_paused.map(|b| quote! { .start_paused(#b) });

        quote! {
            #builder_fn
                #worker_threads
                #max_blocking
                #stack_size
                #thread_name
                #gqi
                #ei
                #keep_alive
                #start_paused
                .enable_all()
                .build()
                .expect("failed to build r2e runtime")
        }
    }
}

pub(crate) fn parse_bool(meta: &syn::meta::ParseNestedMeta) -> syn::Result<bool> {
    let b: syn::LitBool = meta.value()?.parse()?;
    Ok(b.value)
}

pub(crate) fn parse_int<T: std::str::FromStr>(meta: &syn::meta::ParseNestedMeta) -> syn::Result<T>
where
    T::Err: std::fmt::Display,
{
    let i: syn::LitInt = meta.value()?.parse()?;
    i.base10_parse()
}

pub(crate) fn parse_str(meta: &syn::meta::ParseNestedMeta) -> syn::Result<String> {
    let s: syn::LitStr = meta.value()?.parse()?;
    Ok(s.value())
}

/// Returns `true` if `ty` is a path type whose last segment is `name`
/// (matches both `TestApp` and `r2e_test::TestApp`).
pub(crate) fn type_ends_with(ty: &syn::Type, name: &str) -> bool {
    match ty {
        syn::Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|seg| seg.ident == name)
            .unwrap_or(false),
        _ => false,
    }
}
