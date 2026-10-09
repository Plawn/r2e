//! `TenantError` → `Rejection` (error projection, #1072): kinds, configured
//! statuses, the kept cause.

use std::sync::Arc;

use r2e_core::error::{Rejection, RejectionKind};
use r2e_core::http::StatusCode;
use r2e_tenant::{TenantError, TenantId, TenantStatuses};

fn tenant() -> TenantId {
    TenantId::from_static("acme")
}

fn unavailable() -> TenantError {
    TenantError::unavailable(tenant(), "pool exhausted".into())
}

#[test]
fn default_statuses_follow_the_kind_table() {
    let cases = [
        (
            TenantError::Unresolved,
            RejectionKind::BadRequest,
            StatusCode::BAD_REQUEST,
        ),
        (
            TenantError::Unknown(tenant()),
            RejectionKind::NotFound,
            StatusCode::NOT_FOUND,
        ),
        (
            unavailable(),
            RejectionKind::Unavailable,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            TenantError::Timeout(tenant()),
            RejectionKind::Timeout,
            StatusCode::GATEWAY_TIMEOUT,
        ),
        (
            TenantError::Cycle("A -> B -> A".into()),
            RejectionKind::Internal,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            TenantError::NoSource("Pool"),
            RejectionKind::Internal,
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];
    for (err, kind, status) in cases {
        let debug = format!("{err:?}");
        let message = err.to_string();
        let rejection = Rejection::from(err);
        assert_eq!(rejection.kind, kind, "{debug}");
        assert_eq!(rejection.status, status, "{debug}");
        assert_eq!(rejection.message, message, "{debug}: message is Display");
    }
}

#[test]
fn configured_statuses_override_the_defaults_but_keep_the_kind() {
    let statuses = TenantStatuses {
        missing: StatusCode::UNAUTHORIZED,
        unknown: StatusCode::FORBIDDEN,
        unavailable: StatusCode::BAD_GATEWAY,
    };

    let r = TenantError::Unresolved.into_rejection(statuses);
    assert_eq!((r.kind, r.status), (RejectionKind::BadRequest, StatusCode::UNAUTHORIZED));

    let r = TenantError::Unknown(tenant()).into_rejection(statuses);
    assert_eq!((r.kind, r.status), (RejectionKind::NotFound, StatusCode::FORBIDDEN));

    let r = unavailable().into_rejection(statuses);
    assert_eq!((r.kind, r.status), (RejectionKind::Unavailable, StatusCode::BAD_GATEWAY));

    // Timeout and the bugs are not configurable.
    let r = TenantError::Timeout(tenant()).into_rejection(statuses);
    assert_eq!(r.status, StatusCode::GATEWAY_TIMEOUT);
    let r = TenantError::NoSource("Pool").into_rejection(statuses);
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn from_impl_uses_the_default_statuses() {
    let via_from = Rejection::from(TenantError::Unknown(tenant()));
    let via_default = TenantError::Unknown(tenant()).into_rejection(TenantStatuses::default());
    assert_eq!(via_from.kind, via_default.kind);
    assert_eq!(via_from.status, via_default.status);
    assert_eq!(via_from.message, via_default.message);
}

#[test]
fn unavailable_keeps_its_cause_as_the_source() {
    let err = unavailable();
    let cause: Arc<dyn std::error::Error + Send + Sync> = match &err {
        TenantError::Unavailable { source, .. } => Arc::clone(source),
        other => panic!("expected Unavailable, got {other:?}"),
    };
    let rejection = Rejection::from(err);
    let source = rejection.source.expect("cause is kept");
    assert!(Arc::ptr_eq(&source, &cause), "same Arc, not a re-wrap");
    assert_eq!(source.to_string(), "pool exhausted");
}

#[test]
fn other_variants_carry_no_source() {
    for err in [
        TenantError::Unresolved,
        TenantError::Unknown(tenant()),
        TenantError::Timeout(tenant()),
        TenantError::Cycle("A -> A".into()),
        TenantError::NoSource("Pool"),
    ] {
        let debug = format!("{err:?}");
        assert!(Rejection::from(err).source.is_none(), "{debug}");
    }
}

#[test]
fn rejection_and_http_error_agree_on_the_status() {
    let statuses = TenantStatuses {
        missing: StatusCode::UNAUTHORIZED,
        unknown: StatusCode::FORBIDDEN,
        unavailable: StatusCode::BAD_GATEWAY,
    };
    for err in [
        TenantError::Unresolved,
        TenantError::Unknown(tenant()),
        unavailable(),
        TenantError::Timeout(tenant()),
        TenantError::Cycle("A -> A".into()),
        TenantError::NoSource("Pool"),
    ] {
        let debug = format!("{err:?}");
        let rejection = err.clone().into_rejection(statuses);
        let http = err.into_http_error(statuses);
        assert_eq!(rejection.status, http.status(), "{debug}");
    }
}
