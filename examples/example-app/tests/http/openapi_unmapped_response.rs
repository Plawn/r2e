//! End-to-end: the `#[routes]` macro records an unmappable successful response
//! body in `RouteInfo.response_unmapped`, and `r2e_openapi::spec_warnings`
//! surfaces it (the boot-time warning seam).

use r2e::http::response::IntoResponse;
use r2e::http::Json;
use r2e::prelude::*;
use r2e::r2e_core::meta::{MetaRegistry, RouteInfo};
use r2e::r2e_core::HNil;
use r2e::r2e_openapi::{spec_warnings, SchemaGap};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
struct Widget {
    id: u32,
}

#[controller(path = "/w")]
struct WidgetController {}

#[routes]
impl WidgetController {
    // Mapped: Json<Widget> with a JsonSchema — no warning.
    #[get("/ok")]
    async fn ok(&self) -> Json<Widget> {
        Json(Widget { id: 1 })
    }

    // Unmappable: `impl IntoResponse` — the macro cannot see the body type.
    #[get("/raw")]
    async fn raw(&self) -> impl IntoResponse {
        "hello"
    }
}

fn route_metadata<W, C: ControllerTrait<HNil, W>>() -> Vec<RouteInfo> {
    let mut reg = MetaRegistry::new();
    C::register_meta(&mut reg);
    reg.take::<RouteInfo>()
}

#[test]
fn impl_trait_response_is_flagged_as_unmapped() {
    let routes = route_metadata::<_, WidgetController>();

    let raw = routes
        .iter()
        .find(|r| r.path == "/w/raw")
        .expect("raw route present");
    assert_eq!(
        raw.response_unmapped.as_deref(),
        Some("impl IntoResponse"),
        "impl-Trait return recorded for the warning"
    );

    let ok = routes
        .iter()
        .find(|r| r.path == "/w/ok")
        .expect("ok route present");
    assert_eq!(ok.response_unmapped, None, "Json<T> body is mapped");

    let warnings = spec_warnings(&routes);
    assert!(
        warnings.iter().any(|w| w.path == "/w/raw"
            && matches!(&w.gap, SchemaGap::MissingResponseBody { type_name } if type_name == "impl IntoResponse")),
        "spec_warnings flags the unmapped response; got {warnings:?}"
    );
    assert!(
        !warnings.iter().any(|w| w.path == "/w/ok"),
        "the mapped route is not flagged"
    );
}
