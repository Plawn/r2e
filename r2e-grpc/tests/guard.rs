//! gRPC guard bridge: `Rejection` → `tonic::Status` projection by kind, the
//! `GuardContext` built from a `tonic::Request`, and a `Guard<I>` check
//! running end-to-end over request metadata.

use std::future::Future;

use r2e_core::http::{HeaderName, HeaderValue, Method, StatusCode};
use r2e_core::{Guard, GuardContext, Identity, NoIdentity, Rejection, RejectionKind};
use r2e_grpc::{code_from_status, guard_context, rejection_to_status};
use tonic::{Code, Request};

#[test]
fn rejection_kinds_map_to_grpc_codes() {
    let table = [
        (RejectionKind::Unauthenticated, Code::Unauthenticated),
        (RejectionKind::Forbidden, Code::PermissionDenied),
        (RejectionKind::NotFound, Code::NotFound),
        (RejectionKind::Conflict, Code::Aborted),
        (RejectionKind::RateLimited, Code::ResourceExhausted),
        (RejectionKind::PayloadTooLarge, Code::ResourceExhausted),
        (RejectionKind::Unavailable, Code::Unavailable),
        (RejectionKind::Timeout, Code::DeadlineExceeded),
        (RejectionKind::Internal, Code::Internal),
        (RejectionKind::MissingContentType, Code::InvalidArgument),
        (RejectionKind::UnsupportedMediaType, Code::InvalidArgument),
        (RejectionKind::BodyRead, Code::InvalidArgument),
        (RejectionKind::MalformedBody, Code::InvalidArgument),
        (RejectionKind::InvalidBody, Code::InvalidArgument),
        (RejectionKind::InvalidPath, Code::InvalidArgument),
        (RejectionKind::InvalidQuery, Code::InvalidArgument),
        (RejectionKind::InvalidForm, Code::InvalidArgument),
        (RejectionKind::InvalidHeader, Code::InvalidArgument),
        (RejectionKind::BadRequest, Code::InvalidArgument),
        (RejectionKind::Validation, Code::InvalidArgument),
    ];
    for (kind, code) in table {
        let status = rejection_to_status(Rejection::new(kind, "boom"));
        assert_eq!(status.code(), code, "{kind:?}");
        assert_eq!(status.message(), "boom", "{kind:?}");
    }
}

#[test]
fn empty_message_falls_back_to_the_status() {
    let status = rejection_to_status(Rejection::new(RejectionKind::Forbidden, ""));
    assert_eq!(status.code(), Code::PermissionDenied);
    assert_eq!(status.message(), "request rejected with status 403");
}

#[test]
fn rejection_headers_travel_as_status_metadata() {
    let rejection = Rejection::new(RejectionKind::RateLimited, "slow down").header(
        HeaderName::from_static("retry-after"),
        HeaderValue::from_static("30"),
    );
    let status = rejection_to_status(rejection);
    assert_eq!(status.code(), Code::ResourceExhausted);
    assert_eq!(
        status.metadata().get("retry-after").map(|v| v.to_str().unwrap()),
        Some("30")
    );
}

#[test]
fn unknown_kinds_fall_back_on_the_http_status() {
    // `from_status` keeps the status but classifies unlisted 4xx/5xx; the
    // gRPC code then follows the status-only table.
    let teapot = rejection_to_status(Rejection::from_status(StatusCode::IM_A_TEAPOT, "tea"));
    assert_eq!(teapot.code(), Code::InvalidArgument);
    let gateway = rejection_to_status(Rejection::from_status(StatusCode::BAD_GATEWAY, "up"));
    assert_eq!(gateway.code(), Code::Internal);
}

#[test]
fn status_only_table() {
    let table = [
        (StatusCode::UNAUTHORIZED, Code::Unauthenticated),
        (StatusCode::FORBIDDEN, Code::PermissionDenied),
        (StatusCode::NOT_FOUND, Code::NotFound),
        (StatusCode::CONFLICT, Code::Aborted),
        (StatusCode::TOO_MANY_REQUESTS, Code::ResourceExhausted),
        (StatusCode::BAD_REQUEST, Code::InvalidArgument),
        (StatusCode::IM_A_TEAPOT, Code::InvalidArgument),
        (StatusCode::SERVICE_UNAVAILABLE, Code::Unavailable),
        (StatusCode::GATEWAY_TIMEOUT, Code::DeadlineExceeded),
        (StatusCode::INTERNAL_SERVER_ERROR, Code::Internal),
        (StatusCode::BAD_GATEWAY, Code::Internal),
    ];
    for (status, code) in table {
        assert_eq!(code_from_status(status), code, "{status}");
    }
}

// ── GuardContext from a tonic::Request ───────────────────────────────────

struct TestIdentity {
    sub: String,
}

impl Identity for TestIdentity {
    fn sub(&self) -> &str {
        &self.sub
    }
}

fn request_with(key: &'static str, value: &'static str) -> Request<()> {
    let mut request = Request::new(());
    request.metadata_mut().insert(key, value.parse().unwrap());
    request
}

#[test]
fn guard_context_exposes_metadata_as_headers() {
    let request = request_with("x-tenant-id", "acme");
    let identity = TestIdentity {
        sub: "user-1".into(),
    };
    let ctx = guard_context(&request, "say_hello", "Greeter", Some(&identity));

    assert_eq!(ctx.method_name, "say_hello");
    assert_eq!(ctx.controller_name, "Greeter");
    assert_eq!(*ctx.method, Method::POST);
    assert_eq!(ctx.uri.path(), "/");
    assert_eq!(
        ctx.headers.get("x-tenant-id").map(|v| v.to_str().unwrap()),
        Some("acme")
    );
    assert!(ctx.peer_addr.is_none());
    assert!(ctx.path_params.get("id").is_none());
    assert_eq!(ctx.identity.map(|i| i.sub()), Some("user-1"));
}

#[test]
fn guard_context_without_identity() {
    let request = Request::new(());
    let ctx = guard_context::<_, NoIdentity>(&request, "m", "C", None);
    assert!(ctx.identity.is_none());
    assert!(ctx.headers.is_empty());
}

// ── A Guard<I> running over gRPC metadata ────────────────────────────────

/// Header-driven guard: passes when `x-api-key` matches, independent of the
/// identity — the shape of a transport-neutral `Guard<I>`.
struct ApiKeyGuard(&'static str);

impl<I: Identity> Guard<I> for ApiKeyGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Rejection>> + Send {
        let ok = ctx
            .headers
            .get("x-api-key")
            .is_some_and(|v| v.as_bytes() == self.0.as_bytes());
        async move {
            if ok {
                Ok(())
            } else {
                Err(Rejection::forbidden("missing or invalid x-api-key"))
            }
        }
    }
}

#[tokio::test]
async fn guard_denial_projects_onto_a_status() {
    let guard = ApiKeyGuard("s3cret");

    let denied = Request::new(());
    let ctx = guard_context::<_, NoIdentity>(&denied, "m", "C", None);
    let rejection = Guard::check(&guard, &ctx).await.unwrap_err();
    assert_eq!(rejection.kind, RejectionKind::Forbidden);
    let status = rejection_to_status(rejection);
    assert_eq!(status.code(), Code::PermissionDenied);
    assert!(status.message().contains("x-api-key"), "{status}");

    let allowed = request_with("x-api-key", "s3cret");
    let ctx = guard_context::<_, NoIdentity>(&allowed, "m", "C", None);
    Guard::check(&guard, &ctx).await.unwrap();
}
