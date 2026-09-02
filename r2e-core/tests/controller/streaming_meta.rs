//! Streaming routes publish the same OpenAPI metadata as verb routes.
//!
//! `#[sse]` / `#[ws]` used to emit a hardcoded `summary` ("SSE stream" /
//! "WebSocket endpoint") and no description at all, so moving a documented
//! `#[get]` to `#[sse]` silently emptied its entry in the published spec. The
//! generic label is now only a fallback for an undocumented method.
//!
//! The same held for parameters (Tasker #1013): the streaming metadata
//! hardcoded `params: vec![]`, so a `#[derive(Params)]` argument that the
//! handler really does extract never reached `/openapi.json`. Streaming routes
//! now go through the same `params_expr` as verb routes.

use std::convert::Infallible;

use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, ParamLocation, RouteInfo};
use r2e_core::http::extract::Path;
use r2e_core::http::response::SseEvent;
use r2e_core::type_list::HNil;
use r2e_macros::{controller, routes, Params};
use serde::Deserialize;

/// A query DTO on a streaming route — extracted for real by the generated
/// handler, so it belongs in the spec.
#[derive(Params, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamFilter {
    /// Required query parameter.
    run_id: String,
    /// Optional → `required: false`.
    since: Option<String>,
}

#[controller(path = "/streams")]
struct StreamsController {}

#[routes]
impl StreamsController {
    /// Live run events.
    ///
    /// One event per state transition, until the server drains.
    #[sse("/runs")]
    async fn documented(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        r2e_core::rt::stream::empty()
    }

    #[sse("/raw")]
    async fn undocumented(&self) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        r2e_core::rt::stream::empty()
    }

    #[sse("/filtered/{tenant}")]
    async fn filtered(
        &self,
        Path(tenant): Path<String>,
        filter: StreamFilter,
    ) -> impl futures_core::Stream<Item = Result<SseEvent, Infallible>> {
        let _ = (tenant, filter.run_id, filter.since);
        r2e_core::rt::stream::empty()
    }
}

#[cfg(feature = "ws")]
#[controller(path = "/sockets")]
struct SocketsController {}

#[cfg(feature = "ws")]
#[routes]
impl SocketsController {
    /// Terminal session.
    #[ws("/shell")]
    async fn documented(&self, _ws: r2e_core::web::ws::WsStream) {}

    #[ws("/raw")]
    async fn undocumented(&self, _ws: r2e_core::web::ws::WsStream) {}

    #[ws("/filtered")]
    async fn filtered(&self, filter: StreamFilter, _ws: r2e_core::web::ws::WsStream) {
        let _ = (filter.run_id, filter.since);
    }
}

fn route(routes: &[RouteInfo], path: &str) -> RouteInfo {
    routes
        .iter()
        .find(|r| r.path == path)
        .unwrap_or_else(|| panic!("no route for {path} in {:?}", paths(routes)))
        .clone()
}

fn paths(routes: &[RouteInfo]) -> Vec<&str> {
    routes.iter().map(|r| r.path.as_str()).collect()
}

fn sse_routes() -> Vec<RouteInfo> {
    let mut registry = MetaRegistry::new();
    <StreamsController as Controller<HNil, _>>::register_meta(&mut registry);
    registry.take::<RouteInfo>()
}

#[cfg(feature = "ws")]
fn ws_routes() -> Vec<RouteInfo> {
    let mut registry = MetaRegistry::new();
    <SocketsController as Controller<HNil, _>>::register_meta(&mut registry);
    registry.take::<RouteInfo>()
}

#[test]
fn sse_route_publishes_its_doc_comment() {
    let routes = sse_routes();
    let documented = route(&routes, "/streams/runs");

    assert_eq!(documented.summary.as_deref(), Some("Live run events."));
    assert_eq!(
        documented.description.as_deref(),
        Some("One event per state transition, until the server drains.")
    );
}

#[test]
fn undocumented_sse_route_keeps_the_generic_label() {
    let routes = sse_routes();
    let undocumented = route(&routes, "/streams/raw");

    assert_eq!(undocumented.summary.as_deref(), Some("SSE stream"));
    assert_eq!(undocumented.description, None);
}

#[cfg(feature = "ws")]
#[test]
fn ws_route_publishes_its_doc_comment_with_a_websocket_fallback() {
    let routes = ws_routes();

    let documented = route(&routes, "/sockets/shell");
    assert_eq!(documented.summary.as_deref(), Some("Terminal session."));
    assert_eq!(documented.description, None);

    let undocumented = route(&routes, "/sockets/raw");
    assert_eq!(undocumented.summary.as_deref(), Some("WebSocket endpoint"));
}

// ── #[derive(Params)] on a streaming route (Tasker #1013) ─────────────────

/// `(name, location, type, required)` — the readable shape of a `ParamInfo`.
fn shape(route: &RouteInfo) -> Vec<(&str, ParamLocation, &str, bool)> {
    route
        .params
        .iter()
        .map(|p| {
            (
                p.name.as_str(),
                p.location,
                p.param_type.as_str(),
                p.required,
            )
        })
        .collect()
}

#[test]
fn sse_route_publishes_its_params() {
    let routes = sse_routes();
    let filtered = route(&routes, "/streams/filtered/{tenant}");

    assert_eq!(
        shape(&filtered),
        vec![
            ("tenant", ParamLocation::Path, "string", true),
            ("runId", ParamLocation::Query, "string", true),
            ("since", ParamLocation::Query, "string", false),
        ]
    );
}

#[test]
fn sse_route_without_params_publishes_none() {
    let routes = sse_routes();
    assert!(route(&routes, "/streams/runs").params.is_empty());
    assert!(route(&routes, "/streams/raw").params.is_empty());
}

#[cfg(feature = "ws")]
#[test]
fn ws_route_publishes_its_params_and_never_the_socket() {
    let routes = ws_routes();
    let filtered = route(&routes, "/sockets/filtered");

    assert_eq!(
        shape(&filtered),
        vec![
            ("runId", ParamLocation::Query, "string", true),
            ("since", ParamLocation::Query, "string", false),
        ]
    );
}

#[cfg(feature = "ws")]
#[test]
fn ws_route_without_params_publishes_none() {
    let routes = ws_routes();
    assert!(route(&routes, "/sockets/shell").params.is_empty());
    assert!(route(&routes, "/sockets/raw").params.is_empty());
}
