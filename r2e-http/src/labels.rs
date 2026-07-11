//! Bounded label values for HTTP telemetry (metrics and tracing).
//!
//! Telemetry backends key series/tags on label values, so every label derived
//! from a request must stay bounded under attacker-controlled traffic. These
//! helpers are the single source of those semantics, shared by
//! `r2e-prometheus` and `r2e-observability` so the two backends cannot drift.

use crate::extract::MatchedPath;
use crate::header::Method;

/// Route label value for requests no route matched (404s and
/// `Router::fallback`-handled requests carry no [`MatchedPath`]).
/// A single sentinel keeps label cardinality bounded under arbitrary-path
/// scanner traffic.
pub const UNMATCHED_PATH_LABEL: &str = "unmatched";

/// Method label value for non-standard HTTP methods. `http::Method` accepts
/// arbitrary extension tokens, so labeling with the raw method would leave
/// series cardinality attacker-controlled even with a bounded route label.
pub const OTHER_METHOD_LABEL: &str = "other";

/// Bound the method label to the nine standard HTTP methods + one sentinel.
pub fn method_label(method: &Method) -> &'static str {
    match method.as_str() {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "POST" => "POST",
        "PUT" => "PUT",
        "DELETE" => "DELETE",
        "CONNECT" => "CONNECT",
        "OPTIONS" => "OPTIONS",
        "TRACE" => "TRACE",
        "PATCH" => "PATCH",
        _ => OTHER_METHOD_LABEL,
    }
}

/// Route label: the matched route template (`/users/{id}`) — bounded by the
/// number of registered routes — or [`UNMATCHED_PATH_LABEL`] when no route
/// matched.
pub fn route_label(matched_path: Option<&MatchedPath>) -> &str {
    matched_path
        .map(MatchedPath::as_str)
        .unwrap_or(UNMATCHED_PATH_LABEL)
}
