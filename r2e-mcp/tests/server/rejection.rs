//! `From<Rejection> for McpError`: the MCP leg of the #1072 error-projection
//! model. A guard / identity `Rejection` is projected by **kind** (the HTTP
//! status is only a fallback for unknown kinds).

use r2e_core::http::StatusCode;
use r2e_core::{Rejection, RejectionKind};
use r2e_mcp::McpError;
use serde_json::json;

fn project(kind: RejectionKind) -> McpError {
    McpError::from(Rejection::new(kind, "boom"))
}

#[test]
fn auth_kinds_map_to_their_protocol_errors() {
    assert!(matches!(project(RejectionKind::Unauthenticated), McpError::Unauthorized(m) if m == "boom"));
    assert!(matches!(project(RejectionKind::Forbidden), McpError::Forbidden(m) if m == "boom"));
    assert!(matches!(project(RejectionKind::NotFound), McpError::NotFound(m) if m == "boom"));
}

#[test]
fn request_shape_kinds_map_to_invalid_params() {
    for kind in [
        RejectionKind::MissingContentType,
        RejectionKind::UnsupportedMediaType,
        RejectionKind::PayloadTooLarge,
        RejectionKind::BodyRead,
        RejectionKind::MalformedBody,
        RejectionKind::InvalidBody,
        RejectionKind::InvalidPath,
        RejectionKind::InvalidQuery,
        RejectionKind::InvalidForm,
        RejectionKind::InvalidHeader,
        RejectionKind::BadRequest,
        RejectionKind::Validation,
    ] {
        assert!(
            matches!(project(kind), McpError::InvalidParams(m) if m == "boom"),
            "{kind:?}"
        );
    }
}

#[test]
fn server_side_kinds_map_to_internal() {
    for kind in [
        RejectionKind::Internal,
        RejectionKind::Unavailable,
        RejectionKind::Timeout,
    ] {
        assert!(
            matches!(project(kind), McpError::Internal(m) if m == "boom"),
            "{kind:?}"
        );
    }
}

#[test]
fn conflict_and_rate_limit_are_tool_results_with_details() {
    let rejection = Rejection::new(RejectionKind::RateLimited, "slow down")
        .details(json!({"retry_after": 30}));
    match McpError::from(rejection) {
        McpError::Tool { message, data } => {
            assert_eq!(message, "slow down");
            assert_eq!(data, Some(json!({"retry_after": 30})));
        }
        other => panic!("expected Tool, got {other:?}"),
    }
    assert!(matches!(project(RejectionKind::Conflict), McpError::Tool { .. }));
}

#[test]
fn empty_message_falls_back_to_the_status() {
    match McpError::from(Rejection::new(RejectionKind::Forbidden, "")) {
        McpError::Forbidden(m) => assert_eq!(m, "request rejected with status 403"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unlisted_statuses_keep_their_classification() {
    // `from_status` classifies a 418 as `BadRequest` while keeping the
    // status: the kind drives the projection.
    assert!(matches!(
        McpError::from(Rejection::from_status(StatusCode::IM_A_TEAPOT, "tea")),
        McpError::InvalidParams(_)
    ));
    assert!(matches!(
        McpError::from(Rejection::from_status(StatusCode::BAD_GATEWAY, "up")),
        McpError::Internal(_)
    ));
}
