//! Error responses from the route's rejection kinds and its envelope
//! (`ErrorSchema`) — #1072 P2.
//!
//! The builder documents one response per distinct `status_of(kind)` over
//! `RouteInfo::rejection_kinds`, plus the envelope's `extra_statuses`, with
//! the envelope's body schema as the component. The envelope is the route's
//! own (`RouteInfo::error_schema`, from a `Result<T, E>` return type), else
//! the config's (`with_error_schema`, set by the plugin from the
//! application's `ErrorProjector`), else `HttpError`.

use r2e_core::di::meta::{ResponseContent, RouteInfo};
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
        request_body: None,
        request_body_unmapped: None,
        response_status: 200,
        response_unmapped: None,
        response_contents: Vec::new(),
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
fn no_kinds_still_documents_the_panic_500() {
    // The catch-panic layer answers on every route, whatever its kinds.
    let routes = vec![route("GET", "/ping", vec![])];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    let resp = responses(&spec, "/ping", "get");
    assert_eq!(statuses(resp), ["200", "500"]);
    assert_eq!(
        body_ref(&resp["500"])["$ref"],
        "#/components/schemas/ErrorResponse"
    );
}

#[test]
fn validation_and_malformed_share_400_as_any_of() {
    // `HttpError` renders `Validation` with a `details` array
    // (`ValidationErrorResponse`) and every other 400 kind as `ErrorResponse`.
    let routes = vec![route(
        "POST",
        "/items",
        vec![RejectionKind::MalformedBody, RejectionKind::Validation],
    )];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    let four_hundred = &responses(&spec, "/items", "post")["400"];

    let any_of = body_ref(four_hundred)["anyOf"]
        .as_array()
        .expect("two bodies on 400 → anyOf");
    let mut refs: Vec<&str> = any_of.iter().map(|r| r["$ref"].as_str().unwrap()).collect();
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

    // `Wire` on the route: remap + its own extra 502; `Bare` elsewhere. The
    // panic 500 is the app envelope's (`Bare`) on both.
    assert_eq!(statuses(responses(&spec, "/wire", "post")), ["200", "400", "500", "502"]);
    assert_eq!(statuses(responses(&spec, "/app", "post")), ["200", "422", "500"]);
    assert!(responses(&spec, "/wire", "post")["500"].get("content").is_none());
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

        #[post("/form")]
        async fn submit(&self, r2e_core::http::Form(_f): r2e_core::http::Form<ThingForm>) -> &'static str {
            "ok"
        }
    }

    #[derive(serde::Deserialize, schemars::JsonSchema)]
    #[allow(dead_code)]
    struct ThingForm {
        name: String,
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

    /// A form body that does not deserialize is a 422 (`InvalidBody`), with
    /// the media-type and size failures of any body.
    #[r2e_core::test]
    async fn form_body_route_documents_422() {
        let router = AppBuilder::new()
            .plugin(OpenApiPlugin::new(OpenApiConfig::new("t", "1")))
            .build_state()
            .await
            .register_controller::<ThingsController>()
            .build();
        let spec = spec(router).await;
        let resp = &spec["paths"]["/things/form"]["post"]["responses"];

        assert_eq!(statuses(resp), ["201", "400", "413", "415", "422", "500"]);
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

// ── Panic 500: the application envelope, not the route's (S9) ────────────

/// A route envelope remapping `Internal` to 503.
struct Unavailable;

impl ErrorSchema for Unavailable {
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind {
            RejectionKind::Internal => StatusCode::SERVICE_UNAVAILABLE,
            k => k.default_status(),
        }
    }

    fn body_schema() -> Option<(String, Value)> {
        Some(("UnavailableError".to_string(), json!({ "type": "object" })))
    }
}

#[test]
fn panic_500_uses_the_app_envelope_when_the_route_remaps_internal() {
    let routes = vec![RouteInfo {
        error_schema: Some(ErrorSchemaInfo::of::<Unavailable>()),
        ..route("GET", "/x", vec![RejectionKind::Internal])
    }];
    // Default app projector: `HttpError`.
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    let resp = responses(&spec, "/x", "get");

    assert_eq!(statuses(resp), ["200", "500", "503"]);
    assert_eq!(
        body_ref(&resp["500"])["$ref"],
        "#/components/schemas/ErrorResponse"
    );
    assert_eq!(
        body_ref(&resp["503"])["$ref"],
        "#/components/schemas/UnavailableError"
    );
}

#[test]
fn panic_500_merges_as_any_of_when_the_route_envelope_differs() {
    let routes = vec![RouteInfo {
        error_schema: Some(ErrorSchemaInfo::of::<Wire>()),
        ..route("GET", "/x", vec![RejectionKind::Internal])
    }];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    let refs: Vec<&Value> = body_ref(&responses(&spec, "/x", "get")["500"])["anyOf"]
        .as_array()
        .expect("anyOf on 500")
        .iter()
        .map(|r| &r["$ref"])
        .collect();
    assert_eq!(
        refs,
        [
            "#/components/schemas/WireError",
            "#/components/schemas/ErrorResponse"
        ]
    );
}

#[test]
fn panic_500_is_a_single_body_when_route_and_app_envelopes_agree() {
    let routes = vec![RouteInfo {
        error_schema: Some(ErrorSchemaInfo::of::<Wire>()),
        ..route("GET", "/x", vec![RejectionKind::Internal])
    }];
    let config = OpenApiConfig::new("t", "1").with_error_schema::<Wire>();
    let spec = build_spec(&config, &routes);
    let five_hundred = body_ref(&responses(&spec, "/x", "get")["500"]);

    assert!(five_hundred.get("anyOf").is_none());
    assert_eq!(five_hundred["$ref"], "#/components/schemas/WireError");
}

// ── Nested error bodies: `$defs` promoted like any schema (S6) ───────────

#[derive(serde::Serialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct NestedDetail {
    field: String,
    reason: String,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct NestedErrorBody {
    error: String,
    detail: NestedDetail,
}

struct NestedEnvelope;

impl ErrorSchema for NestedEnvelope {
    fn body_schema() -> Option<(String, Value)> {
        let schema = schemars::schema_for!(NestedErrorBody);
        Some((
            "NestedErrorBody".to_string(),
            serde_json::to_value(schema).expect("schema json"),
        ))
    }
}

fn collect_refs<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::Object(obj) => {
            if let Some(Value::String(r)) = obj.get("$ref") {
                out.push(r);
            }
            obj.values().for_each(|v| collect_refs(v, out));
        }
        Value::Array(arr) => arr.iter().for_each(|v| collect_refs(v, out)),
        _ => {}
    }
}

#[test]
fn nested_error_body_refs_all_resolve() {
    let routes = vec![route("GET", "/x", vec![RejectionKind::Forbidden])];
    let config = OpenApiConfig::new("t", "1").with_error_schema::<NestedEnvelope>();
    let spec = build_spec(&config, &routes);
    let schemas = &spec["components"]["schemas"];

    let mut refs = Vec::new();
    collect_refs(&spec, &mut refs);
    assert!(
        refs.contains(&"#/components/schemas/NestedDetail"),
        "the nested type is referenced: {refs:?}"
    );
    for r in refs {
        let name = r
            .strip_prefix("#/components/schemas/")
            .unwrap_or_else(|| panic!("non-component ref {r}"));
        assert!(schemas.get(name).is_some(), "dangling $ref {r}");
    }
    assert!(schemas["NestedErrorBody"].get("$defs").is_none());
}

// ── Error components: schema-equality dedup, collisions inlined (S9/N1/N2) ─

/// `HttpError`'s plain body, verbatim, under another name.
struct AliasEnvelope;

impl ErrorSchema for AliasEnvelope {
    fn body_schema() -> Option<(String, Value)> {
        r2e_core::HttpError::body_schema().map(|(_, schema)| ("Problem".to_string(), schema))
    }
}

/// A route envelope whose body reuses `HttpError`'s component name with a
/// different shape.
struct CodeEnvelope;

impl ErrorSchema for CodeEnvelope {
    fn body_schema() -> Option<(String, Value)> {
        Some((
            "ErrorResponse".to_string(),
            json!({
                "type": "object",
                "properties": { "code": { "type": "integer" } },
                "required": ["code"]
            }),
        ))
    }
}

fn inline_warnings(warnings: &[r2e_openapi::SpecWarning]) -> Vec<(String, String, String)> {
    warnings
        .iter()
        .filter_map(|w| match &w.gap {
            r2e_openapi::SchemaGap::ErrorBodyInlined { component } => {
                Some((w.method.clone(), w.path.clone(), component.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn equal_bodies_under_two_names_document_once() {
    // Route envelope `Problem` and the app's panic `ErrorResponse` are the
    // same schema: one `$ref` (the first-recorded, the route's), no union.
    let routes = vec![RouteInfo {
        error_schema: Some(ErrorSchemaInfo::of::<AliasEnvelope>()),
        ..route("GET", "/x", vec![RejectionKind::Internal])
    }];
    let (spec, warnings) =
        r2e_openapi::build_spec_with_warnings(&OpenApiConfig::new("t", "1"), &routes);
    let five_hundred = body_ref(&responses(&spec, "/x", "get")["500"]);

    assert!(five_hundred.get("anyOf").is_none(), "{five_hundred}");
    assert_eq!(five_hundred["$ref"], "#/components/schemas/Problem");
    assert!(spec["components"]["schemas"].get("ErrorResponse").is_none());
    assert!(inline_warnings(&warnings).is_empty());
}

#[test]
fn colliding_app_body_is_inlined_next_to_the_route_body() {
    // Route `ErrorResponse{code}` vs the app's `HttpError` `ErrorResponse`:
    // the route envelope owns the component, the app body is inlined.
    let routes = vec![RouteInfo {
        error_schema: Some(ErrorSchemaInfo::of::<CodeEnvelope>()),
        ..route("GET", "/x", vec![RejectionKind::Internal])
    }];
    let (spec, warnings) =
        r2e_openapi::build_spec_with_warnings(&OpenApiConfig::new("t", "1"), &routes);
    let any_of = body_ref(&responses(&spec, "/x", "get")["500"])["anyOf"]
        .as_array()
        .expect("anyOf on 500")
        .clone();

    let app_body = r2e_core::HttpError::body_schema().unwrap().1;
    assert_eq!(
        any_of,
        [json!({ "$ref": "#/components/schemas/ErrorResponse" }), app_body]
    );
    assert_eq!(
        spec["components"]["schemas"]["ErrorResponse"],
        CodeEnvelope::body_schema().unwrap().1
    );
    assert_eq!(
        inline_warnings(&warnings),
        [("GET".to_string(), "/x".to_string(), "ErrorResponse".to_string())]
    );
    let msg = warnings
        .iter()
        .find(|w| matches!(w.gap, r2e_openapi::SchemaGap::ErrorBodyInlined { .. }))
        .unwrap()
        .message();
    assert!(msg.contains("/x") && msg.contains("ErrorResponse"), "{msg}");
}

/// A success DTO nesting a type named like the framework's error body.
#[derive(serde::Serialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct Report {
    errors: Vec<ErrorResponse>,
}

#[derive(serde::Serialize, schemars::JsonSchema)]
#[allow(dead_code)]
struct ErrorResponse {
    line: u32,
    text: String,
}

#[test]
fn nested_dto_keeps_its_component_and_the_error_body_is_inlined() {
    let report = serde_json::to_value(schemars::schema_for!(Report)).unwrap();
    let routes = vec![RouteInfo {
        response_contents: vec![ResponseContent::json(Some(("Report".to_string(), report)))],
        ..route(
            "POST",
            "/reports",
            vec![RejectionKind::MalformedBody, RejectionKind::Validation],
        )
    }];
    let (spec, warnings) =
        r2e_openapi::build_spec_with_warnings(&OpenApiConfig::new("t", "1"), &routes);
    let schemas = &spec["components"]["schemas"];

    // The DTO owns `ErrorResponse`; `Report` still resolves to it.
    assert!(schemas["ErrorResponse"]["properties"]["line"].is_object(), "{schemas}");
    let mut refs = Vec::new();
    collect_refs(&schemas["Report"], &mut refs);
    assert_eq!(refs, ["#/components/schemas/ErrorResponse"]);

    // The framework body is inlined on 400 (beside the validation `$ref`)
    // and on 500.
    let resp = responses(&spec, "/reports", "post");
    let app_body = r2e_core::HttpError::body_schema().unwrap().1;
    assert_eq!(
        body_ref(&resp["400"])["anyOf"],
        json!([app_body, { "$ref": "#/components/schemas/ValidationErrorResponse" }])
    );
    assert_eq!(*body_ref(&resp["500"]), app_body);
    assert_eq!(
        inline_warnings(&warnings),
        [("POST".to_string(), "/reports".to_string(), "ErrorResponse".to_string())]
    );
}

// ── Runtime bodies validate against the documented schemas ─────────────────

/// A minimal JSON Schema matcher for the subset the error components use:
/// `$ref` (to `components/schemas`), `anyOf`, `type`, `properties`,
/// `required`, `items`.
fn matches(schema: &Value, value: &Value, components: &Value) -> bool {
    if let Some(r) = schema["$ref"].as_str() {
        let name = r.strip_prefix("#/components/schemas/").expect("component ref");
        return matches(&components[name], value, components);
    }
    if let Some(branches) = schema["anyOf"].as_array() {
        return branches.iter().any(|b| matches(b, value, components));
    }
    let type_ok = match schema["type"].as_str() {
        None => true,
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some(other) => panic!("matcher: unsupported type {other}"),
    };
    if !type_ok {
        return false;
    }
    if let Some(required) = schema["required"].as_array() {
        if required.iter().any(|k| value.get(k.as_str().unwrap()).is_none()) {
            return false;
        }
    }
    if let Some(props) = schema["properties"].as_object() {
        for (k, sub) in props {
            if let Some(v) = value.get(k) {
                if !matches(sub, v, components) {
                    return false;
                }
            }
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), value.as_array()) {
        if !arr.iter().all(|v| matches(items, v, components)) {
            return false;
        }
    }
    true
}

/// The indices of the `anyOf` branches `value` matches.
fn matching_branches(schema: &Value, value: &Value, components: &Value) -> Vec<usize> {
    schema["anyOf"]
        .as_array()
        .expect("anyOf")
        .iter()
        .enumerate()
        .filter(|(_, b)| matches(b, value, components))
        .map(|(i, _)| i)
        .collect()
}

async fn rendered(err: r2e_core::HttpError) -> Value {
    use http_body_util::BodyExt;
    use r2e_core::http::response::IntoResponse;
    let bytes = err.into_response().into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn rendered_bodies_validate_against_the_documented_any_of() {
    use r2e_core::web::validation::{FieldError, ValidationErrorResponse};
    let plain = rendered(r2e_core::HttpError::BadRequest("x".into())).await;
    let validation = rendered(r2e_core::HttpError::validation(ValidationErrorResponse {
        errors: vec![FieldError {
            field: "name".into(),
            message: "too short".into(),
            code: "validation".into(),
        }],
    }))
    .await;
    assert_eq!(plain, json!({ "error": "x" }));

    // Default envelope, 400: [ErrorResponse, ValidationErrorResponse].
    let routes = vec![route(
        "POST",
        "/items",
        vec![RejectionKind::MalformedBody, RejectionKind::Validation],
    )];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    let components = &spec["components"]["schemas"];
    let four_hundred = body_ref(&responses(&spec, "/items", "post")["400"]);
    let names: Vec<&str> = four_hundred["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["$ref"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "#/components/schemas/ErrorResponse",
            "#/components/schemas/ValidationErrorResponse"
        ]
    );
    // The plain body is only an `ErrorResponse`; the validation body is
    // both — which is why the union is `anyOf` (a `oneOf` rejects it).
    assert_eq!(matching_branches(four_hundred, &plain, components), [0]);
    assert_eq!(matching_branches(four_hundred, &validation, components), [0, 1]);
    assert!(matches(four_hundred, &plain, components));
    assert!(matches(four_hundred, &validation, components));

    // Colliding route envelope, 500: [route `ErrorResponse{code}` ref, inlined
    // app body]. Each body matches exactly its own branch.
    let routes = vec![RouteInfo {
        error_schema: Some(ErrorSchemaInfo::of::<CodeEnvelope>()),
        ..route("GET", "/x", vec![RejectionKind::Internal])
    }];
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    let components = &spec["components"]["schemas"];
    let five_hundred = body_ref(&responses(&spec, "/x", "get")["500"]);
    let panic_body = rendered(r2e_core::HttpError::Internal("boom".into())).await;
    assert_eq!(matching_branches(five_hundred, &json!({ "code": 7 }), components), [0]);
    assert_eq!(matching_branches(five_hundred, &panic_body, components), [1]);
}
