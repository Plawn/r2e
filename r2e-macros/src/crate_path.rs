//! Cached crate-path resolution: `proc_macro_crate::crate_name` re-parses
//! `Cargo.toml` on every call, so each path is resolved once per process.
//!
//! We cache the rendered `String` (not a `TokenStream`, which is `!Send + !Sync`
//! in the real proc-macro backend) and re-parse it on each hit. The parse is a
//! few tokens; the avoided Cargo.toml walk is the win.

use std::sync::OnceLock;

use proc_macro2::TokenStream;
use proc_macro_crate::{crate_name, FoundCrate};

/// `(candidate_crate_name, suffix_within_that_crate)`. First match wins.
/// `suffix` is appended after `crate` / `::<name>` when non-empty, so the
/// facade crate (`r2e`) can re-export subcrates under a module path.
type Candidate = (&'static str, &'static str);

fn render_found(found: &FoundCrate, suffix: &str) -> String {
    match found {
        FoundCrate::Itself => {
            if suffix.is_empty() {
                "crate".to_string()
            } else {
                format!("crate::{}", suffix)
            }
        }
        FoundCrate::Name(name) => {
            if suffix.is_empty() {
                format!("::{}", name)
            } else {
                format!("::{}::{}", name, suffix)
            }
        }
    }
}

fn resolve_cached(
    cache: &'static OnceLock<String>,
    candidates: &[Candidate],
    fallback: &str,
) -> TokenStream {
    let rendered = cache.get_or_init(|| {
        for (crate_candidate, suffix) in candidates {
            if let Ok(found) = crate_name(crate_candidate) {
                return render_found(&found, suffix);
            }
        }
        fallback.to_string()
    });
    rendered
        .parse()
        .expect("r2e-macros: cached crate path must be valid Rust")
}

fn resolve_cached_optional(
    cache: &'static OnceLock<Option<String>>,
    candidates: &[Candidate],
) -> Option<TokenStream> {
    let rendered = cache.get_or_init(|| {
        for (crate_candidate, suffix) in candidates {
            if let Ok(found) = crate_name(crate_candidate) {
                return Some(render_found(&found, suffix));
            }
        }
        None
    });
    rendered
        .as_ref()
        .map(|s| s.parse().expect("r2e-macros: cached crate path must be valid Rust"))
}

/// Returns the token stream for accessing `r2e_core` types.
///
/// If the user depends on `r2e`, returns `::r2e`.
/// Otherwise returns `::r2e_core`.
pub fn r2e_core_path() -> TokenStream {
    static CACHE: OnceLock<String> = OnceLock::new();
    resolve_cached(&CACHE, &[("r2e", ""), ("r2e-core", "")], "::r2e_core")
}

/// Returns the token stream for accessing `r2e_security` types.
pub fn r2e_security_path() -> TokenStream {
    static CACHE: OnceLock<String> = OnceLock::new();
    resolve_cached(
        &CACHE,
        &[("r2e", "r2e_security"), ("r2e-security", "")],
        "::r2e_security",
    )
}

/// Returns the token stream for accessing `r2e_events` types.
pub fn r2e_events_path() -> TokenStream {
    static CACHE: OnceLock<String> = OnceLock::new();
    resolve_cached(
        &CACHE,
        &[("r2e", "r2e_events"), ("r2e-events", "")],
        "::r2e_events",
    )
}

/// Returns the token stream for accessing `r2e_scheduler` types.
pub fn r2e_scheduler_path() -> TokenStream {
    static CACHE: OnceLock<String> = OnceLock::new();
    resolve_cached(
        &CACHE,
        &[("r2e", "r2e_scheduler"), ("r2e-scheduler", "")],
        "::r2e_scheduler",
    )
}

/// Returns the token stream for accessing `r2e_devtools` types.
pub fn r2e_devtools_path() -> TokenStream {
    static CACHE: OnceLock<String> = OnceLock::new();
    resolve_cached(
        &CACHE,
        &[("r2e", "devtools"), ("r2e-devtools", "")],
        "::r2e_devtools",
    )
}

/// Returns the token stream for accessing `schemars` through `r2e-openapi`.
///
/// Resolution order:
/// 1. Direct `schemars` dependency → `::schemars`
/// 2. Direct `r2e-openapi` dependency → `::r2e_openapi::schemars`
///
/// Returns `None` if no path is found (i.e. user hasn't opted into OpenAPI).
pub fn r2e_schemars_path() -> Option<TokenStream> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    resolve_cached_optional(&CACHE, &[("schemars", ""), ("r2e-openapi", "schemars")])
}

/// Returns the token stream for accessing `r2e_grpc` types.
pub fn r2e_grpc_path() -> TokenStream {
    static CACHE: OnceLock<String> = OnceLock::new();
    resolve_cached(
        &CACHE,
        &[("r2e", "r2e_grpc"), ("r2e-grpc", "")],
        "::r2e_grpc",
    )
}
