use r2e::http::extract::Request;
use r2e::http::response::{IntoResponse, Response};
use r2e::http::{Body, StatusCode};
use r2e::prelude::*;

/// Proxy-shaped routing: `#[any]` + `#[fallback]` give registry-proxy /
/// gateway apps first-class controller routes instead of a raw-axum fallback.
///
/// - `#[any("/proxy/{*path}")]` matches every HTTP method under `/proxy/…` and
///   receives the raw `Request` (method, URI, headers, streaming body).
/// - `#[fallback]` handles every request no other route in the app matched —
///   root-mounted controllers only, at most one per app.
///
/// Both are excluded from the OpenAPI spec (no single documentable shape), and
/// both still get DI (`#[inject]`/`#[config]`), guards, and interceptors.
#[controller]
pub struct ProxyController {
    #[config("app.name")]
    app_name: String,
}

#[routes]
impl ProxyController {
    /// Streaming pass-through echo, patina-style: authenticates in-handler
    /// (per-protocol auth doesn't fit a declarative guard), then streams the
    /// request body straight back — no buffering.
    #[any("/proxy/{*path}")]
    async fn proxy(&self, req: Request) -> Response {
        if req.headers().get("x-proxy-key").is_none() {
            return (
                StatusCode::UNAUTHORIZED,
                [("www-authenticate", "ProxyKey")],
                "missing x-proxy-key",
            )
                .into_response();
        }
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let body = req.into_body(); // streaming — never buffered
        Response::builder()
            .status(StatusCode::OK)
            .header("x-proxied-by", &self.app_name)
            .header("x-proxy-method", method.as_str())
            .header("x-proxy-path", path)
            .body(Body::new(body))
            .unwrap()
    }

    /// App-wide catch-all: anything no controller matched lands here.
    #[fallback]
    async fn not_found(&self, req: Request) -> Response {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no such route",
                "method": req.method().as_str(),
                "path": req.uri().path(),
                "app": self.app_name,
            })),
        )
            .into_response()
    }
}
