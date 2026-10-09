//! Error responses from the route's rejection kinds and its envelope
//! (`ErrorSchema`) — #1072 P2.
//!
//! The builder documents one response per distinct `status_of(kind)` over
//! `RouteInfo::rejection_kinds`, plus the envelope's `extra_statuses`, with
//! the envelope's body schema as the component. The envelope is the route's
//! own (`RouteInfo::error_schema`, from a `Result<T, E>` return type), else
//! the config's (`with_error_schema`, set by the plugin from the
//! application's `ErrorProjector`), else `HttpError`.

use r2e_core::di::meta::RouteInfo;
use r2e_core::http::StatusCode;
use r2e_core::{ErrorSchema, ErrorSchemaInfo, RejectionKind};
use r2e_openapi::{build_spec, OpenApiConfig};
use serde_json::{json, Value};

fn route(method: &str, path: &str, kinds: Vec<RejectionKind>) -> RouteInfo {
    RouteInfo {
        path: path.to_string(),
        method: method.to_string(),
        operation_id: format!("{}_{}", method.to_lowercase(), path.trim_start_matches('/')),
        summary: None,
        description: None,
        request_body_type: None,
        request_body_schema: None,
        request_body_content_type: None,
        request_body_required: true,
        response_type: None,
        response_schema: None,
        response_status: 200,
        response_unmapped: None,
        params: vec![],
        roles: vec![],
        tag: None,
        deprecated: false,
        rejection_kinds: kinds,
        error_schema: None,
    }
}

fn responses<'a>(spec: &'a Value, path: &str, method: &str) -> &'a Value {
    &spec["paths"][path][method]["responses"]
}

fn statuses(responses: &Value) -> Vec<String> {
    let mut keys: Vec<String> = responses
        .as_object()
        .expect("responses object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

fn body_ref(response: &Value) -> &Value {
    &response["content"]["application/json"]["schema"]
}

/// A foreign envelope: `InvalidBody` remapped to 400, an own body component,
/// and a 502 no inferred kind covers.
struct Wire;

impl ErrorSchema for Wire {
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind {
            RejectionKind::InvalidBody => StatusCode::BAD_REQUEST,
            k => k.default_status(),
        }
    }

    fn body_schema() -> Option<(String, Value)> {
        Some((
            "WireError".to_string(),
            json!({
                "type": "object",
                "properties": { "error": { "type": "object" } }
            }),
        ))
    }

    fn extra_statuses() -> Vec<(StatusCode, &'static str)> {
        vec![(StatusCode::BAD_GATEWAY, "upstream failed")]
    }
}

/// An envelope with no documented body.
struct Bare;

impl ErrorSchema for Bare {
    fn body_schema() -> Option<(String, Value)> {
        None
    }
}

// ── Default envelope (HttpError) ───────────────────────────────────────────

#[test]
fn each_kind_documents_at_its_default_status() {
    use RejectionKind::*;
    let routes = vec![route(
        "POST",
        "/items",
        vec![
            MissingContentType,
            PayloadTooLarge,
            MalformedBody,
            InvalidBody,
            Unauthenticated,
            Forbidden,
            RateLimited,
            Internal,
        ],
    )];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    let resp = responses(&spec, "/items", "post");

    assert_eq!(
        statuses(resp),
        ["200", "400", "401", "403", "413", "415", "422", "429", "500"]
    );
    assert_eq!(resp["415"]["description"], "Unsupported Media Type");
    assert_eq!(resp["422"]["description"], "Unprocessable Entity");
    assert_eq!(
        body_ref(&resp["500"])["$ref"],
        "#/components/schemas/ErrorResponse"
    );
    assert!(spec["components"]["schemas"]["ErrorResponse"].is_object());
}

#[test]
fn no_kinds_means_only_the_success_response() {
    let routes = vec![route("GET", "/ping", vec![])];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    assert_eq!(statuses(responses(&spec, "/ping", "get")), ["200"]);
    assert!(spec["components"]["schemas"].get("ErrorResponse").is_none());
}

#[test]
fn validation_and_malformed_share_400_as_one_of() {
    // `HttpError` renders `Validation` with a `details` array
    // (`ValidationErrorResponse`) and every other 400 kind as `ErrorResponse`.
    let routes = vec![route(
        "POST",
        "/items",
        vec![RejectionKind::MalformedBody, RejectionKind::Validation],
    )];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    let four_hundred = &responses(&spec, "/items", "post")["400"];

    let one_of = body_ref(four_hundred)["oneOf"]
        .as_array()
        .expect("two bodies on 400 → oneOf");
    let mut refs: Vec<&str> = one_of.iter().map(|r| r["$ref"].as_str().unwrap()).collect();
    refs.sort();
    assert_eq!(
        refs,
        [
            "#/components/schemas/ErrorResponse",
            "#/components/schemas/ValidationErrorResponse"
        ]
    );
    let validation = &spec["components"]["schemas"]["ValidationErrorResponse"];
    assert!(validation["properties"]["details"].is_object());
    // The field-error items are inline: no separate `FieldError` component.
    assert!(spec["components"]["schemas"].get("FieldError").is_none());
}

#[test]
fn validation_alone_is_a_plain_ref() {
    let routes = vec![route("POST", "/items", vec![RejectionKind::Validation])];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    assert_eq!(
        body_ref(&responses(&spec, "/items", "post")["400"])["$ref"],
        "#/components/schemas/ValidationErrorResponse"
    );
}

#[test]
fn success_status_wins_over_a_colliding_error_status() {
    // A route answering 400 on success (odd, but declared) keeps its own
    // response object for that status.
    let routes = vec![RouteInfo {
        response_status: 400,
        ..route("GET", "/odd", vec![RejectionKind::MalformedBody])
    }];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    assert_eq!(
        responses(&spec, "/odd", "get")["400"]["description"],
        "Successful response"
    );
}

// ── Custom envelopes ───────────────────────────────────────────────────────

#[test]
fn config_envelope_remaps_statuses_and_adds_extra_ones() {
    let routes = vec![route(
        "POST",
        "/items",
        vec![RejectionKind::InvalidBody, RejectionKind::Internal],
    )];
    let config = OpenApiConfig::new("t", "1").with_error_schema::<Wire>();
    let spec = build_spec(&config, &routes);
    let resp = responses(&spec, "/items", "post");

    // InvalidBody → 400 (remapped), no 422; 502 from `extra_statuses`.
    assert_eq!(statuses(resp), ["200", "400", "500", "502"]);
    assert_eq!(resp["502"]["description"], "Bad Gateway");
    for status in ["400", "500", "502"] {
        assert_eq!(
            body_ref(&resp[status])["$ref"],
            "#/components/schemas/WireError",
            "{status}"
        );
    }
    assert!(spec["components"]["schemas"]["WireError"].is_object());
    assert!(spec["components"]["schemas"].get("ErrorResponse").is_none());
}

#[test]
fn route_envelope_overrides_the_config_envelope() {
    let routes = vec![
        RouteInfo {
            error_schema: Some(ErrorSchemaInfo::of::<Wire>()),
            ..route("POST", "/wire", vec![RejectionKind::InvalidBody])
        },
        route("POST", "/app", vec![RejectionKind::InvalidBody]),
    ];
    let config = OpenApiConfig::new("t", "1").with_error_schema::<Bare>();
    let spec = build_spec(&config, &routes);

    // `Wire` on the route: remap + its own extra 502; `Bare` elsewhere.
    assert_eq!(statuses(responses(&spec, "/wire", "post")), ["200", "400", "502"]);
    assert_eq!(statuses(responses(&spec, "/app", "post")), ["200", "422"]);
}

#[test]
fn envelope_without_body_documents_description_only() {
    let routes = vec![route("GET", "/x", vec![RejectionKind::Internal])];
    let config = OpenApiConfig::new("t", "1").with_error_schema::<Bare>();
    let spec = build_spec(&config, &routes);
    let five_hundred = &responses(&spec, "/x", "get")["500"];

    assert_eq!(five_hundred["description"], "Internal Server Error");
    assert!(five_hundred.get("content").is_none());
    assert!(spec["components"].get("schemas").is_none());
}

#[test]
fn schema_override_beats_the_envelope_component() {
    let routes = vec![route("GET", "/x", vec![RejectionKind::Internal])];
    let config = OpenApiConfig::new("t", "1")
        .with_error_schema::<Wire>()
        .with_schema_override("WireError", json!({ "type": "string" }));
    let spec = build_spec(&config, &routes);
    assert_eq!(spec["components"]["schemas"]["WireError"], json!({ "type": "string" }));
}

#[test]
fn error_schema_info_reads_through_to_the_trait() {
    let info = ErrorSchemaInfo::of::<Wire>();
    assert_eq!(info.type_name(), std::any::type_name::<Wire>());
    assert_eq!(
        info.status_of(RejectionKind::InvalidBody),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        info.status_of(RejectionKind::Forbidden),
        StatusCode::FORBIDDEN
    );
    assert_eq!(info.extra_statuses(), vec![(StatusCode::BAD_GATEWAY, "upstream failed")]);
    assert!(!info.opaque_passthrough());
}

// ── Plugin: the application's projector documents the routes ──────────────

mod plugin {
    use super::*;
    use http_body_util::BodyExt;
    use r2e_core::http::response::{IntoHttpResponse, IntoResponse};
    use r2e_core::http::{Body, Request, Response};
    use r2e_core::builder::RegisterController;
    use r2e_core::{AppBuilder, Rejection};
    use r2e_macros::{controller, routes};
    use r2e_openapi::OpenApiPlugin;
    use tower::ServiceExt;

    /// The app-level envelope: an own component, no remap.
    #[derive(Debug)]
    struct AppEnvelope(Rejection);

    impl From<Rejection> for AppEnvelope {
        fn from(r: Rejection) -> Self {
            Self(r)
        }
    }

    impl IntoHttpResponse for AppEnvelope {
        fn into_http_response(self) -> Response {
            (self.0.status, self.0.message.into_owned()).into_response()
        }
    }

    r2e_core::http::impl_into_response!(AppEnvelope);

    impl ErrorSchema for AppEnvelope {
        fn body_schema() -> Option<(String, Value)> {
            Some(("AppError".to_string(), json!({ "type": "object" })))
        }
    }

    #[controller(path = "/things")]
    struct ThingsController {}

    #[routes]
    impl ThingsController {
        #[get("/{id}")]
        async fn get(&self, r2e_core::http::extract::Path(_id): r2e_core::http::extract::Path<u32>) -> &'static str {
            "thing"
        }
    }

    async fn spec(router: r2e_core::http::Router) -> Value {
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[r2e_core::test]
    async fn plugin_documents_with_the_error_projector_bean() {
        let router = AppBuilder::new()
            .error_projection::<AppEnvelope>()
            .plugin(OpenApiPlugin::new(OpenApiConfig::new("t", "1")))
            .build_state()
            .await
            .register_controller::<ThingsController>()
            .build();
        let spec = spec(router).await;
        let resp = &spec["paths"]["/things/{id}"]["get"]["responses"];

        assert_eq!(statuses(resp), ["200", "400", "500"]);
        assert_eq!(
            body_ref(&resp["400"])["$ref"],
            "#/components/schemas/AppError"
        );
        assert!(spec["components"]["schemas"].get("ErrorResponse").is_none());
    }

    #[r2e_core::test]
    async fn plugin_falls_back_to_http_error_without_a_projector() {
        let router = AppBuilder::new()
            .plugin(OpenApiPlugin::new(OpenApiConfig::new("t", "1")))
            .build_state()
            .await
            .register_controller::<ThingsController>()
            .build();
        let spec = spec(router).await;
        let resp = &spec["paths"]["/things/{id}"]["get"]["responses"];

        assert_eq!(statuses(resp), ["200", "400", "500"]);
        assert_eq!(
            body_ref(&resp["500"])["$ref"],
            "#/components/schemas/ErrorResponse"
        );
    }
}
