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

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::spanned::Spanned;

/// The `rt::RuntimeBuilder` knobs shared by every runtime-driving macro.
///
/// `flavor`: `None` = user did not set it, `Some(true)` = current_thread,
/// `Some(false)` = multi_thread (the default).
///
/// Every knob remembers the span of the value it was parsed from, so
/// [`validate`](RuntimeArgs::validate) can reject the combinations the runtime
/// builder *panics* on — a panic inside the builder happens before any
/// `.expect(...)` we could attach, so those must be caught at macro time or
/// they surface as a bare `Worker threads cannot be set to 0` with no hint of
/// which test or suite asked for it.
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
    spans: Spans,
}

/// Spans of the values that [`RuntimeArgs::validate`] can point an error at.
#[derive(Default)]
struct Spans {
    worker_threads: Option<Span>,
    max_blocking_threads: Option<Span>,
    thread_name: Option<Span>,
    global_queue_interval: Option<Span>,
    event_interval: Option<Span>,
    start_paused: Option<Span>,
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
            "worker_threads" => {
                self.spans.worker_threads = Some(meta.path.span());
                self.worker_threads = Some(parse_int(meta)?);
            }
            "max_blocking_threads" => {
                self.spans.max_blocking_threads = Some(meta.path.span());
                self.max_blocking_threads = Some(parse_int(meta)?);
            }
            "thread_stack_size" => self.thread_stack_size = Some(parse_int(meta)?),
            "thread_name" => {
                self.spans.thread_name = Some(meta.path.span());
                self.thread_name = Some(parse_str(meta)?);
            }
            "global_queue_interval" => {
                self.spans.global_queue_interval = Some(meta.path.span());
                self.global_queue_interval = Some(parse_int(meta)?);
            }
            "event_interval" => {
                self.spans.event_interval = Some(meta.path.span());
                self.event_interval = Some(parse_int(meta)?);
            }
            "thread_keep_alive" => self.thread_keep_alive_secs = Some(parse_int(meta)?),
            "start_paused" => {
                self.spans.start_paused = Some(meta.path.span());
                self.start_paused = Some(parse_bool(meta)?);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Reject the knob combinations the runtime builder answers with a *panic*.
    ///
    /// Those panics fire inside the builder — a setter's `assert!`, or the
    /// paused clock refusing a multi-thread runtime — i.e. before any
    /// `.build().expect(...)` message we attach can run. Catching them here
    /// turns a bare runtime panic into an error spanned on the offending
    /// argument, which is what the caller can actually act on.
    ///
    /// Call it from every macro that owns a `RuntimeArgs`, right after parsing.
    pub fn validate(&self) -> syn::Result<()> {
        if self.start_paused == Some(true) && self.flavor != Some(true) {
            return Err(syn::Error::new(
                self.span(self.spans.start_paused),
                "`start_paused = true` requires `flavor = \"current_thread\"`: a paused clock is \
                 only supported on the current-thread runtime, and the multi-thread runtime (the \
                 default here) panics while building instead of returning an error",
            ));
        }
        for (value, span, what) in [
            (
                self.worker_threads,
                self.spans.worker_threads,
                "worker_threads",
            ),
            (
                self.max_blocking_threads,
                self.spans.max_blocking_threads,
                "max_blocking_threads",
            ),
        ] {
            if value == Some(0) {
                return Err(syn::Error::new(
                    self.span(span),
                    format!("`{what}` must be greater than 0 — the runtime builder panics on 0"),
                ));
            }
        }
        for (value, span, what) in [
            (
                self.global_queue_interval,
                self.spans.global_queue_interval,
                "global_queue_interval",
            ),
            (
                self.event_interval,
                self.spans.event_interval,
                "event_interval",
            ),
        ] {
            if value == Some(0) {
                return Err(syn::Error::new(
                    self.span(span),
                    format!("`{what}` must be greater than 0 — the runtime builder panics on 0"),
                ));
            }
        }
        if self
            .thread_name
            .as_deref()
            .is_some_and(|n| n.trim().is_empty())
        {
            return Err(syn::Error::new(
                self.span(self.spans.thread_name),
                "`thread_name` must not be empty — the runtime builder panics on a blank name",
            ));
        }
        Ok(())
    }

    fn span(&self, span: Option<Span>) -> Span {
        span.unwrap_or_else(Span::call_site)
    }

    /// Generate the full `rt::RuntimeBuilder` chain including `.build()`.
    /// Defaults to `new_multi_thread()` unless `flavor = "current_thread"`.
    ///
    /// `krate` is the resolved r2e-core root (`::r2e`, `::r2e_core` or
    /// `crate`); the builder is reached through its `rt` re-export.
    pub fn builder_tokens(&self, krate: &TokenStream2) -> TokenStream2 {
        self.builder_tokens_for(krate, "failed to build r2e runtime")
    }

    /// [`builder_tokens`](Self::builder_tokens) with a caller-supplied panic
    /// message, so a failure names *which* runtime could not be built. The
    /// suite macro uses it: a suite whose runtime never materialises must not
    /// look like an ordinary test failure.
    pub fn builder_tokens_for(&self, krate: &TokenStream2, on_error: &str) -> TokenStream2 {
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

        // `catch_unwind`: `build()` reports most problems as an `Err`, but a few
        // knob combinations make the builder *panic* instead (a paused clock on
        // a multi-thread runtime, a zero thread count). `validate` rejects the
        // ones we know at macro time; this is the backstop that keeps the
        // remainder from surfacing as an anonymous tokio panic, so the promise
        // "a runtime that fails to build names its owner" holds either way.
        quote! {
            match ::std::panic::catch_unwind(|| {
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
            }) {
                ::core::result::Result::Ok(::core::result::Result::Ok(__r2e_runtime)) => __r2e_runtime,
                ::core::result::Result::Ok(::core::result::Result::Err(__r2e_err)) => {
                    ::std::panic!("{}: {}", #on_error, __r2e_err)
                }
                ::core::result::Result::Err(_) => {
                    ::std::panic!("{}: the runtime builder panicked (its own message is above)", #on_error)
                }
            }
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
