//! Tower-http layers behind an R2E name.
//!
//! `tower-http` is the HTTP backend's middleware companion. It is confined to
//! this crate — plus `r2e-grpc`'s grpc-web arm, which fronts tonic rather than
//! the HTTP router — so a backend change has one place to re-source these
//! from. The types are tower-http's own: identity is preserved, a
//! [`cors::CorsLayer`] built here is the very type `tower-http` hands out, and
//! the two interoperate freely.
//!
//! `tower` itself is deliberately **not** behind this module. `Layer` /
//! `Service` are the neutral middleware vocabulary, like `http` and `bytes`,
//! and stay public (plans/axum-containment-roadmap.md, decision D1).
//!
//! ```rust,ignore
//! use r2e::http::layers::timeout::TimeoutLayer;
//! use std::time::Duration;
//!
//! #[get("/slow")]
//! #[layer(TimeoutLayer::new(Duration::from_secs(2)))]
//! async fn slow(&self) -> Json<&'static str> { Json("done") }
//! ```

use crate::Routes;

pub use tower_http::cors;
pub use tower_http::normalize_path;
pub use tower_http::set_header;
pub use tower_http::timeout;

/// A permissive CORS layer: any origin, any method, any header.
///
/// Suitable for development or internal services. For production, configure
/// the `Cors` plugin (`Cors::new()` reads the `cors.*` section; explicit
/// origins through `cors.allowed-origins` or its builder setters) — this
/// layer is what `Cors::permissive()` installs.
pub fn default_cors() -> cors::CorsLayer {
    cors::CorsLayer::new()
        .allow_origin(cors::Any)
        .allow_methods(cors::Any)
        .allow_headers(cors::Any)
}

// ── Origin allowlist grammar ────────────────────────────────────────────────
//
// One grammar for every origin allowlist the framework reads (`cors.*`,
// `mcp.allowed-origins`, `mcp.cors.allowed-origins`): an entry is `*`, an
// exact origin (`scheme://host[:port]`), or `scheme://host:*` ("any port on
// that host"). Validation happens once at boot; matching is a plain string
// comparison per request.

/// Validate one allowlist entry.
///
/// Accepted forms: `*`, `scheme://host`, `scheme://host:port`,
/// `scheme://host:*`. Rejected: a missing scheme, an empty host, a port that
/// is neither a number nor `*`, and anything carrying a path, query or
/// fragment (an origin never does — `http://app.example.com/` is the typo
/// this catches). The error message names the offending entry so a boot
/// failure points at the config line to fix.
pub fn validate_origin(entry: &str) -> Result<(), String> {
    if entry == "*" {
        return Ok(());
    }
    let bad = |why: &str| Err(format!("origin {entry:?} is invalid: {why}"));
    let Some((scheme, rest)) = entry.split_once("://") else {
        return bad("expected `scheme://host[:port]` (or `*`)");
    };
    let scheme_ok = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !scheme_ok {
        return bad("the scheme must start with a letter");
    }
    if rest.is_empty() {
        return bad("the host is empty");
    }
    if rest.contains(['/', '?', '#', ' ']) {
        return bad("an origin has no path, query or fragment");
    }
    // Split host from port; bracketed IPv6 literals keep their colons.
    let (host, port) = if let Some(after) = rest.strip_prefix('[') {
        let Some((inside, tail)) = after.split_once(']') else {
            return bad("unterminated IPv6 literal");
        };
        if inside.is_empty() {
            return bad("the host is empty");
        }
        match tail.strip_prefix(':') {
            Some(port) => (inside, Some(port)),
            None if tail.is_empty() => (inside, None),
            None => return bad("unexpected characters after the IPv6 literal"),
        }
    } else {
        match rest.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        }
    };
    if host.is_empty() {
        return bad("the host is empty");
    }
    match port {
        None | Some("*") => Ok(()),
        Some(port) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => Ok(()),
        Some(_) => bad("the port must be a number or `*`"),
    }
}

/// Does `origin` (the request's `Origin` header value) match one of the
/// `allowed` entries? Exact match, or a `:*` suffix matching any port
/// (`http://localhost:*` accepts `http://localhost:5173`). A `*` entry
/// matches everything.
pub fn origin_allowed(allowed: &[String], origin: &str) -> bool {
    allowed.iter().any(|entry| {
        if entry == "*" {
            true
        } else if let Some(prefix) = entry.strip_suffix(":*") {
            origin
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
        } else {
            entry == origin
        }
    })
}

/// The [`cors::AllowOrigin`] for an allowlist: [`cors::Any`] when the list
/// holds `*`, otherwise a predicate over [`origin_allowed`] (the matched
/// origin is mirrored back, as the spec requires for credentialed requests).
///
/// Entries are expected to be [`validate_origin`]-clean; an unvalidated,
/// malformed entry simply never matches.
pub fn allow_origins(origins: &[String]) -> cors::AllowOrigin {
    if origins.iter().any(|o| o == "*") {
        return cors::AllowOrigin::any();
    }
    let origins: Vec<String> = origins.to_vec();
    cors::AllowOrigin::predicate(move |origin, _| {
        origin
            .to_str()
            .is_ok_and(|origin| origin_allowed(&origins, origin))
    })
}

/// Wrap fully-built routes in pre-routing trailing-slash normalization.
///
/// A middleware added via `Routes::layer` runs after routing and cannot
/// change which route matches, so the rewrite must wrap the routes
/// themselves: the whole tree goes inside tower-http's `NormalizePath`
/// service, and the wrapped service is re-embedded as the fallback of a fresh
/// empty `Routes` so callers keep receiving plain `Routes`. The outer tree is
/// a trivial extra dispatch hop — the meaningful routing (and `MatchedPath`
/// insertion) happens once, in the wrapped inner tree. Anything applied
/// OUTSIDE this wrap therefore never sees `MatchedPath`.
///
/// tower-http's `trim_trailing_slash` also collapses a leading run of
/// slashes (`//admin` → `/admin`).
pub fn normalize_path_router(routes: Routes) -> Routes {
    use tower::Layer as _;
    let svc = normalize_path::NormalizePathLayer::trim_trailing_slash().layer(routes);
    Routes::new().fallback_service(svc)
}
