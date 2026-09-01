//! Streaming routes publish the same OpenAPI prose as verb routes.
//!
//! `#[sse]` / `#[ws]` used to emit a hardcoded `summary` ("SSE stream" /
//! "WebSocket endpoint") and no description at all, so moving a documented
//! `#[get]` to `#[sse]` silently emptied its entry in the published spec. The
//! generic label is now only a fallback for an undocumented method.

use std::convert::Infallible;

use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, RouteInfo};
use r2e_core::http::response::SseEvent;
use r2e_core::type_list::HNil;
use r2e_macros::{controller, routes};

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
    let mut registry = MetaRegistry::new();
    <SocketsController as Controller<HNil, _>>::register_meta(&mut registry);
    let routes = registry.take::<RouteInfo>();

    let documented = route(&routes, "/sockets/shell");
    assert_eq!(documented.summary.as_deref(), Some("Terminal session."));
    assert_eq!(documented.description, None);

    let undocumented = route(&routes, "/sockets/raw");
    assert_eq!(undocumented.summary.as_deref(), Some("WebSocket endpoint"));
}
