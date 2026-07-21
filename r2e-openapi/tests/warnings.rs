use r2e_core::meta::RouteInfo;
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
        has_auth: false,
    }
}

// ── Missing response body (unmappable return type) ──────────────────────────

#[test]
fn warns_on_unmappable_response_body() {
    let routes = vec![RouteInfo {
        response_unmapped: Some("impl IntoResponse".to_string()),
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
}

#[test]
fn no_warning_when_response_body_is_mapped() {
    // A resolved response type with a schema is fully mapped.
    let routes = vec![RouteInfo {
        response_type: Some("User".to_string()),
        response_schema: Some(json!({ "type": "object" })),
        ..base("GET", "/users")
    }];
    assert!(spec_warnings(&routes).is_empty());
}

#[test]
fn no_warning_for_intentional_no_body() {
    // response_unmapped is None (macro did not flag it): no warning even though
    // there is no response_type.
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
        ..base("DELETE", "/thing")
    }];
    assert!(spec_warnings(&routes).is_empty());
}

// ── Schemaless named bodies (no JsonSchema → generic object) ─────────────────

#[test]
fn warns_on_schemaless_response_type() {
    let routes = vec![RouteInfo {
        response_type: Some("User".to_string()),
        response_schema: None,
        ..base("GET", "/users")
    }];

    let warnings = spec_warnings(&routes);
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].gap,
        SchemaGap::SchemalessResponseBody {
            type_name: "User".to_string(),
        }
    );
    assert!(warnings[0].message().contains("JsonSchema"));
}

#[test]
fn warns_on_schemaless_request_type() {
    let routes = vec![RouteInfo {
        request_body_type: Some("CreateUser".to_string()),
        request_body_schema: None,
        ..base("POST", "/users")
    }];

    let warnings = spec_warnings(&routes);
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].gap,
        SchemaGap::SchemalessRequestBody {
            type_name: "CreateUser".to_string(),
        }
    );

    // The message renders the request-body arm: names the route, the type, and
    // points at the `JsonSchema` derive fix.
    let msg = warnings[0].message();
    assert!(msg.contains("POST"));
    assert!(msg.contains("/users"));
    assert!(msg.contains("CreateUser"));
    assert!(msg.contains("request type"));
    assert!(msg.contains("JsonSchema"));
}

#[test]
fn no_warning_for_raw_multipart_body() {
    // Raw multipart bodies carry a content type but no named type — not flagged.
    let routes = vec![RouteInfo {
        request_body_type: None,
        request_body_content_type: Some("multipart/form-data".to_string()),
        request_body_schema: None,
        ..base("POST", "/upload")
    }];
    assert!(spec_warnings(&routes).is_empty());
}

// ── build_spec still produces a valid spec despite gaps ─────────────────────

#[test]
fn build_spec_documents_unmapped_response_without_body() {
    let routes = vec![RouteInfo {
        response_unmapped: Some("Html<String>".to_string()),
        ..base("GET", "/page")
    }];
    let spec = build_spec(&OpenApiConfig::new("Test", "1.0"), &routes);

    // Response is present but body-less (no content) — the documented gap.
    let resp = &spec["paths"]["/page"]["get"]["responses"]["200"];
    assert!(resp.get("description").is_some());
    assert!(resp.get("content").is_none());
}

