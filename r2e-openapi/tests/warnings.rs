use r2e_core::di::meta::{RequestBody, ResponseContent, RouteInfo};
use r2e_openapi::{build_spec, spec_warnings, OpenApiConfig, SchemaGap, SpecWarning};
use serde_json::json;

// ── Helpers ─────────────────────────────────────────────────────────────────

fn base(method: &str, path: &str) -> RouteInfo {
    RouteInfo {
        path: path.to_string(),
        method: method.to_string(),
        operation_id: format!("{method}_{path}"),
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
        rejection_kinds: vec![],
        error_schema: None,
    }
}

// ── Missing request body (unmappable body extractor) ────────────────────────

#[test]
fn warns_on_unmappable_request_body() {
    let routes = vec![RouteInfo {
        request_body_unmapped: Some("Json<Plain>".to_string()),
        ..base("POST", "/items")
    }];
    let warnings = spec_warnings(&routes);
    assert_eq!(
        warnings,
        vec![SpecWarning {
            method: "POST".to_string(),
            path: "/items".to_string(),
            gap: SchemaGap::MissingRequestBody {
                type_name: "Json<Plain>".to_string(),
            },
        }]
    );
    let msg = warnings[0].message();
    assert!(msg.contains("POST /items"), "{msg}");
    assert!(msg.contains("Json<Plain>"), "{msg}");
    assert!(msg.contains("RequestBodySchema"), "{msg}");
    assert!(msg.contains("JsonSchema"), "{msg}");

    // The spec itself still renders, without a requestBody.
    let spec = build_spec(&OpenApiConfig::new("t", "1"), &routes);
    assert!(spec["paths"]["/items"]["post"].get("requestBody").is_none());
}

// ── Missing response body (unmappable return type) ──────────────────────────

#[test]
fn warns_on_unmappable_response_body() {
    let routes = vec![RouteInfo {
        response_unmapped: Some("impl IntoResponse".to_string()),
        response_contents: Vec::new(),
        ..base("GET", "/stream")
    }];

    let warnings = spec_warnings(&routes);
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0],
        SpecWarning {
            method: "GET".to_string(),
            path: "/stream".to_string(),
            gap: SchemaGap::MissingResponseBody {
                type_name: "impl IntoResponse".to_string(),
            },
        }
    );

    // The message names the route, the type, and the opt-out attribute.
    let msg = warnings[0].message();
    assert!(msg.contains("GET"));
    assert!(msg.contains("/stream"));
    assert!(msg.contains("impl IntoResponse"));
    assert!(msg.contains("#[returns(T)]"));
    assert!(msg.contains("ResponseBodySchema"));
}

#[test]
fn no_warning_when_response_body_is_mapped() {
    // A return type with a `ResponseBodySchema` impl is fully mapped.
    let routes = vec![RouteInfo {
        response_contents: vec![ResponseContent::json(Some((
            "User".to_string(),
            json!({ "type": "object" }),
        )))],
        ..base("GET", "/users")
    }];
    assert!(spec_warnings(&routes).is_empty());
}

#[test]
fn no_warning_for_intentional_no_body() {
    // response_unmapped is None (macro did not flag it): no warning even though
    // there are no response contents.
    let routes = vec![
        base("DELETE", "/users/{id}"),
        RouteInfo {
            response_status: 204,
            ..base("POST", "/logout")
        },
    ];
    assert!(spec_warnings(&routes).is_empty());
}

#[test]
fn no_missing_body_warning_at_204_even_if_flagged() {
    // A 204 route never carries a body, so it is not flagged even if the macro
    // recorded an unmapped type (defensive).
    let routes = vec![RouteInfo {
        response_status: 204,
        response_unmapped: Some("Bytes".to_string()),
        response_contents: Vec::new(),
        ..base("DELETE", "/thing")
    }];
    assert!(spec_warnings(&routes).is_empty());
}

// ── Schemaless bodies are not gaps ──────────────────────────────────────────

#[test]
fn no_warning_for_schemaless_bodies() {
    // A body documented from its media type alone (raw `Multipart`, `Bytes`,
    // `String`, a text response) is a deliberate choice of its
    // `RequestBodySchema` / `ResponseBodySchema` impl — not flagged.
    let routes = vec![
        RouteInfo {
            request_body: Some(RequestBody {
                content_type: "multipart/form-data".to_string(),
                schema: None,
                required: true,
            }),
            ..base("POST", "/upload")
        },
        RouteInfo {
            response_contents: vec![ResponseContent::text()],
            ..base("GET", "/plain")
        },
    ];
    assert!(spec_warnings(&routes).is_empty());
}

// ── build_spec still produces a valid spec despite gaps ─────────────────────

#[test]
fn build_spec_documents_unmapped_response_without_body() {
    let routes = vec![RouteInfo {
        response_unmapped: Some("Html<String>".to_string()),
        response_contents: Vec::new(),
        ..base("GET", "/page")
    }];
    let spec = build_spec(&OpenApiConfig::new("Test", "1.0"), &routes);

    // Response is present but body-less (no content) — the documented gap.
    let resp = &spec["paths"]["/page"]["get"]["responses"]["200"];
    assert!(resp.get("description").is_some());
    assert!(resp.get("content").is_none());
}
