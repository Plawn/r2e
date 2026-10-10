//! The `Rejection` hub (`r2e-core/src/error/rejection.rs`): kind → status
//! table, every `From<X> for Rejection` the core crate owns, and projection
//! through an envelope (`Rejection::project::<E>()`), with `HttpError` as the
//! default envelope — whose bodies must stay byte-equal to the 0.4 renderer.

use std::borrow::Cow;

use http_body_util::BodyExt;
use r2e_core::decorators::guards::GuardError;
use r2e_core::error::{ErrorSchema, HttpError, Rejection, RejectionKind};
use r2e_core::http::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use r2e_core::http::header::{HeaderValue, RETRY_AFTER, WWW_AUTHENTICATE};
use r2e_core::http::response::{IntoHttpResponse, IntoResponse, Response};
use r2e_core::http::{Body, Form, Json, StatusCode};
use r2e_core::web::params::{ParamError, ParamLocation};
use r2e_core::web::validation::{FieldError, ValidationErrorResponse};
use serde::Deserialize;

// ── helpers ──────────────────────────────────────────────────────────────

async fn parts(resp: Response) -> (StatusCode, Vec<u8>) {
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, body)
}

async fn json_parts(resp: Response) -> (StatusCode, serde_json::Value) {
    let (status, body) = parts(resp).await;
    (status, serde_json::from_slice(&body).unwrap())
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Payload {
    name: String,
    count: u32,
}

/// A foreign envelope: remaps `Validation` to 422, tags every response with
/// its own header, renders `{"type": kind, "message": ..}`.
struct Envelope(Rejection);

impl From<Rejection> for Envelope {
    fn from(r: Rejection) -> Self {
        Self(r)
    }
}

impl IntoHttpResponse for Envelope {
    fn into_http_response(self) -> Response {
        let r = self.0;
        (
            r.status,
            [("x-envelope", "custom"), ("retry-after", "envelope")],
            Json(serde_json::json!({
                "type": format!("{:?}", r.kind),
                "status": r.status.as_u16(),
                "message": r.message,
                "details": r.details,
            })),
        )
            .into_response()
    }
}

/// An envelope that delegates conversion and rendering to `HttpError` but
/// remaps `Validation` to 422 — the `#[error(transparent)]` shape.
struct Delegating(HttpError);

impl From<Rejection> for Delegating {
    fn from(r: Rejection) -> Self {
        Self(HttpError::from(r))
    }
}

impl IntoHttpResponse for Delegating {
    fn into_http_response(self) -> Response {
        self.0.into_http_response()
    }
}

impl ErrorSchema for Delegating {
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind {
            RejectionKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            other => other.default_status(),
        }
    }

    fn body_schema() -> Option<(String, serde_json::Value)> {
        HttpError::body_schema()
    }
}

impl ErrorSchema for Envelope {
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind {
            RejectionKind::Validation => StatusCode::UNPROCESSABLE_ENTITY,
            other => other.default_status(),
        }
    }

    fn body_schema() -> Option<(String, serde_json::Value)> {
        None
    }
}

// ── kind table ───────────────────────────────────────────────────────────

#[test]
fn kind_default_status_table() {
    use RejectionKind as K;
    let expected = [
        (K::MissingContentType, 415),
        (K::UnsupportedMediaType, 415),
        (K::PayloadTooLarge, 413),
        (K::BodyRead, 400),
        (K::MalformedBody, 400),
        (K::InvalidBody, 422),
        (K::InvalidPath, 400),
        (K::InvalidQuery, 400),
        (K::InvalidForm, 400),
        (K::InvalidHeader, 400),
        (K::BadRequest, 400),
        (K::Validation, 400),
        (K::Unauthenticated, 401),
        (K::Forbidden, 403),
        (K::NotFound, 404),
        (K::MethodNotAllowed, 405),
        (K::Conflict, 409),
        (K::RateLimited, 429),
        (K::Internal, 500),
        (K::Unavailable, 503),
        (K::Timeout, 504),
        (K::Opaque, 500),
    ];
    assert_eq!(expected.len(), K::ALL.len(), "table must list every kind");
    for (kind, status) in expected {
        assert_eq!(kind.default_status().as_u16(), status, "{kind:?}");
        assert!(K::ALL.contains(&kind), "{kind:?} missing from ALL");
    }
}

#[test]
fn from_status_round_trips_every_default_status() {
    for kind in RejectionKind::ALL {
        let status = kind.default_status();
        assert_eq!(
            RejectionKind::from_status(status).default_status(),
            status,
            "{kind:?}"
        );
    }
    assert_eq!(
        RejectionKind::from_status(StatusCode::IM_A_TEAPOT),
        RejectionKind::BadRequest
    );
    assert_eq!(
        RejectionKind::from_status(StatusCode::BAD_GATEWAY),
        RejectionKind::Internal
    );
}

#[test]
fn constructors_carry_the_default_status() {
    let r = Rejection::unauthenticated();
    assert_eq!(r.kind, RejectionKind::Unauthenticated);
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.message, "Unauthorized");

    let r = Rejection::with_status(RejectionKind::BadRequest, StatusCode::IM_A_TEAPOT, "tea");
    assert_eq!(r.status, StatusCode::IM_A_TEAPOT);

    let r = Rejection::from_status(StatusCode::CONFLICT, "dup");
    assert_eq!(r.kind, RejectionKind::Conflict);
    assert_eq!(r.to_string(), "dup");
}

// ── From<axum rejections> ────────────────────────────────────────────────

#[r2e_core::test]
async fn json_rejection_kinds() {
    let r = Rejection::from(Json::<Payload>::from_bytes(b"{bad").unwrap_err());
    assert_eq!(r.kind, RejectionKind::MalformedBody);
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.source.is_some());

    let r = Rejection::from(Json::<Payload>::from_bytes(br#"{"name":"a"}"#).unwrap_err());
    assert_eq!(r.kind, RejectionKind::InvalidBody);
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);

    let req = Request::builder()
        .method("POST")
        .body(Body::from(r#"{"name":"a","count":1}"#))
        .unwrap();
    let rejection = <Json<Payload> as FromRequest<()>>::from_request(req, &())
        .await
        .unwrap_err();
    let r = Rejection::from(rejection);
    assert_eq!(r.kind, RejectionKind::MissingContentType);
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[r2e_core::test]
async fn query_and_form_rejections() {
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct Q {
        n: u32,
    }
    let uri: r2e_core::http::Uri = "/x?n=abc".parse().unwrap();
    let rejection = Query::<Q>::try_from_uri(&uri).err().expect("rejected");
    let r = Rejection::from(rejection);
    assert_eq!(r.kind, RejectionKind::InvalidQuery);
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let req = Request::builder()
        .method("POST")
        .body(Body::from("n=1"))
        .unwrap();
    let rejection = <Form<Q> as FromRequest<()>>::from_request(req, &())
        .await
        .err()
        .expect("rejected");
    let r = Rejection::from(rejection);
    assert_eq!(r.kind, RejectionKind::UnsupportedMediaType);
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

/// `Form<T>` failures by source: a body that does not deserialize is a body
/// failure (`InvalidBody`, 422 — the status axum carries), a query-string form
/// (GET/HEAD) stays `InvalidForm` (400).
#[r2e_core::test]
async fn form_rejection_kind_follows_the_source() {
    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct Q {
        n: u32,
    }
    async fn reject(req: Request) -> Rejection {
        let rejection = <Form<Q> as FromRequest<()>>::from_request(req, &())
            .await
            .err()
            .expect("rejected");
        Rejection::from(rejection)
    }

    let body = Request::builder()
        .method("POST")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from("n=abc"))
        .unwrap();
    let r = reject(body).await;
    assert_eq!(r.kind, RejectionKind::InvalidBody);
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);

    let query = Request::builder()
        .method("GET")
        .uri("/x?n=abc")
        .body(Body::empty())
        .unwrap();
    let r = reject(query).await;
    assert_eq!(r.kind, RejectionKind::InvalidForm);
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    let media = Request::builder()
        .method("POST")
        .header("content-type", "text/plain")
        .body(Body::from("n=1"))
        .unwrap();
    let r = reject(media).await;
    assert_eq!(r.kind, RejectionKind::UnsupportedMediaType);
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[r2e_core::test]
async fn path_rejection_keeps_its_carried_status() {
    // No matched route → axum answers 500 (`MissingPathParams`); the kind
    // is still `InvalidPath`, the status is preserved.
    let (mut parts, _) = Request::builder().body(Body::empty()).unwrap().into_parts();
    let rejection = <Path<u32> as FromRequestParts<()>>::from_request_parts(&mut parts, &())
        .await
        .err()
        .expect("rejected");
    let r = Rejection::from(rejection);
    assert_eq!(r.kind, RejectionKind::InvalidPath);
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
}

// ── From<core fault types> ───────────────────────────────────────────────

#[test]
fn param_error_maps_by_location() {
    for (location, kind) in [
        (ParamLocation::Path, RejectionKind::InvalidPath),
        (ParamLocation::Query, RejectionKind::InvalidQuery),
        (ParamLocation::Header, RejectionKind::InvalidHeader),
    ] {
        let r = Rejection::from(ParamError {
            location,
            message: "bad".into(),
        });
        assert_eq!(r.kind, kind);
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert_eq!(r.message, "bad");
    }
}

fn validation() -> ValidationErrorResponse {
    ValidationErrorResponse {
        errors: vec![FieldError {
            field: "name".into(),
            message: "required".into(),
            code: "required".into(),
        }],
    }
}

#[test]
fn validation_response_becomes_validation_with_details() {
    let r = Rejection::from(validation());
    assert_eq!(r.kind, RejectionKind::Validation);
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "0.4 answered 400");
    assert_eq!(r.message, "Validation failed");
    assert_eq!(r.details.unwrap()[0]["field"], "name");
}

#[test]
fn garde_report_becomes_validation() {
    let mut report = garde::Report::new();
    report.append(garde::Path::new("email"), garde::Error::new("not an email"));
    let r = Rejection::from(&report);
    assert_eq!(r.kind, RejectionKind::Validation);
    let details = r.details.unwrap();
    assert_eq!(details[0]["field"], "email");
    assert_eq!(details[0]["message"], "not an email");
}

#[test]
fn guard_error_keeps_status_and_message() {
    let r = Rejection::from(GuardError::new(StatusCode::IM_A_TEAPOT, "short and stout"));
    assert_eq!(r.kind, RejectionKind::BadRequest);
    assert_eq!(r.status, StatusCode::IM_A_TEAPOT);
    assert_eq!(r.message, "short and stout");
}

#[test]
fn http_error_maps_by_variant() {
    let cases: [(HttpError, RejectionKind, &str); 5] = [
        (HttpError::NotFound("a".into()), RejectionKind::NotFound, "a"),
        (
            HttpError::Unauthorized("b".into()),
            RejectionKind::Unauthenticated,
            "b",
        ),
        (HttpError::Forbidden("c".into()), RejectionKind::Forbidden, "c"),
        (HttpError::BadRequest("d".into()), RejectionKind::BadRequest, "d"),
        (HttpError::Internal("e".into()), RejectionKind::Internal, "e"),
    ];
    for (err, kind, message) in cases {
        let r = Rejection::from(err);
        assert_eq!(r.kind, kind);
        assert_eq!(r.status, kind.default_status());
        assert_eq!(r.message, message);
        assert!(r.details.is_none());
    }

    let r = Rejection::from(HttpError::validation(validation()));
    assert_eq!(r.kind, RejectionKind::Validation);
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // A carried validation status survives the round trip.
    let r = Rejection::from(HttpError::Validation {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        response: validation(),
    });
    assert_eq!(r.kind, RejectionKind::Validation);
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);

    let body = serde_json::json!({"error": "teapot", "code": 7});
    let r = Rejection::from(HttpError::Custom {
        status: StatusCode::IM_A_TEAPOT,
        body: body.clone(),
    });
    assert_eq!(r.kind, RejectionKind::BadRequest);
    assert_eq!(r.status, StatusCode::IM_A_TEAPOT);
    assert_eq!(r.message, "teapot");
    assert_eq!(r.details, Some(body));

    let r = Rejection::from(HttpError::Custom {
        status: StatusCode::BAD_GATEWAY,
        body: serde_json::json!({"upstream": "down"}),
    });
    assert_eq!(r.kind, RejectionKind::Internal);
    assert_eq!(r.message, "Bad Gateway");

    let r = Rejection::from(HttpError::WithSource {
        status: StatusCode::BAD_GATEWAY,
        message: "upstream".into(),
        source: std::sync::Arc::new(std::io::Error::other("disk")),
    });
    assert_eq!(r.kind, RejectionKind::Internal);
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert_eq!(r.message, "upstream");
    assert!(r.source.is_some());
    assert!(std::error::Error::source(&r).is_some());
}

#[test]
fn response_becomes_opaque() {
    let r = Rejection::from((StatusCode::IM_A_TEAPOT, "tea").into_response());
    assert_eq!(r.kind, RejectionKind::Opaque);
    assert_eq!(r.status, StatusCode::IM_A_TEAPOT);
    assert!(r.is_opaque());
}

// ── Rejection → HttpError (default envelope) ─────────────────────────────

#[test]
fn rejection_to_http_error_by_shape() {
    assert!(matches!(
        HttpError::from(Rejection::not_found("x")),
        HttpError::NotFound(m) if m == "x"
    ));
    assert!(matches!(
        HttpError::from(Rejection::unauthenticated()),
        HttpError::Unauthorized(_)
    ));
    assert!(matches!(
        HttpError::from(Rejection::from(validation())),
        HttpError::Validation { status: StatusCode::BAD_REQUEST, response }
            if response.errors.len() == 1
    ));
    assert!(matches!(
        HttpError::from(Rejection::new(RejectionKind::Conflict, "dup")),
        HttpError::Custom { status: StatusCode::CONFLICT, .. }
    ));
    let io = std::io::Error::other("disk");
    assert!(matches!(
        HttpError::from(Rejection::internal("boom").source(io)),
        HttpError::WithSource { status: StatusCode::INTERNAL_SERVER_ERROR, .. }
    ));
    let body = serde_json::json!({"error": "teapot", "code": 7});
    let r = Rejection::from(HttpError::Custom {
        status: StatusCode::IM_A_TEAPOT,
        body: body.clone(),
    });
    assert!(matches!(
        HttpError::from(r),
        HttpError::Custom { status: StatusCode::IM_A_TEAPOT, body: b } if b == body
    ));
}

#[r2e_core::test]
async fn default_projection_is_byte_equal_to_http_error() {
    let cases: Vec<(Rejection, HttpError)> = vec![
        (
            Rejection::not_found("missing"),
            HttpError::NotFound("missing".into()),
        ),
        (
            Rejection::unauthenticated(),
            HttpError::Unauthorized("Unauthorized".into()),
        ),
        (
            Rejection::forbidden("Insufficient roles"),
            HttpError::Forbidden("Insufficient roles".into()),
        ),
        (
            Rejection::new(RejectionKind::MalformedBody, "Failed to parse"),
            HttpError::BadRequest("Failed to parse".into()),
        ),
        (
            Rejection::new(RejectionKind::RateLimited, "Rate limit exceeded"),
            HttpError::from_status(StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded"),
        ),
        (
            Rejection::from(validation()),
            HttpError::validation(validation()),
        ),
        (
            Rejection::internal("boom").source(std::io::Error::other("disk")),
            HttpError::Internal("boom".into()),
        ),
    ];
    for (rejection, error) in cases {
        let expected = parts(error.into_http_response()).await;
        let actual = parts(rejection.project::<HttpError>()).await;
        assert_eq!(actual.0, expected.0);
        assert_eq!(
            String::from_utf8_lossy(&actual.1),
            String::from_utf8_lossy(&expected.1)
        );
    }
}

#[r2e_core::test]
async fn into_http_response_and_from_response_use_the_default_envelope() {
    let (status, body) = json_parts(Rejection::not_found("gone").into_http_response()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "gone");

    let resp: Response = Rejection::not_found("gone").into();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn default_projection_passes_an_opaque_response_through() {
    let original = (
        StatusCode::IM_A_TEAPOT,
        [("x-custom", "yes")],
        "<html>tea</html>",
    )
        .into_response();
    let resp = Rejection::from(original).project::<HttpError>();
    assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(resp.headers()["x-custom"], "yes");
    let (_, body) = parts(resp).await;
    assert_eq!(body, b"<html>tea</html>");
}

#[r2e_core::test]
async fn custom_envelope_renders_an_opaque_rejection_itself() {
    let original = (StatusCode::IM_A_TEAPOT, "<html>tea</html>").into_response();
    let (status, body) = json_parts(Rejection::from(original).project::<Envelope>()).await;
    assert_eq!(status, StatusCode::IM_A_TEAPOT);
    assert_eq!(body["type"], "Opaque");
}

#[r2e_core::test]
async fn hub_headers_are_added_unless_the_envelope_set_them() {
    let r = Rejection::unauthenticated()
        .header(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))
        .header(RETRY_AFTER, HeaderValue::from_static("hub"));

    let resp = Rejection::unauthenticated()
        .header(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))
        .project::<HttpError>();
    assert_eq!(resp.headers()[WWW_AUTHENTICATE], "Bearer");

    let resp = r.project::<Envelope>();
    assert_eq!(resp.headers()[WWW_AUTHENTICATE], "Bearer");
    assert_eq!(resp.headers()["x-envelope"], "custom");
    // The envelope's own `Retry-After` wins over the hub's.
    assert_eq!(resp.headers()[RETRY_AFTER], "envelope");
}

#[r2e_core::test]
async fn hub_headers_are_merged_into_an_opaque_passthrough() {
    let original = (StatusCode::SERVICE_UNAVAILABLE, [("x-custom", "yes")], "down").into_response();
    let resp = Rejection::from(original)
        .header(RETRY_AFTER, HeaderValue::from_static("30"))
        .project::<HttpError>();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(resp.headers()[RETRY_AFTER], "30");
    assert_eq!(resp.headers()["x-custom"], "yes");
}

#[r2e_core::test]
async fn opaque_response_own_header_wins_over_the_hub() {
    let original = (StatusCode::SERVICE_UNAVAILABLE, [(RETRY_AFTER, "5")], "down").into_response();
    let resp = Rejection::from(original)
        .header(RETRY_AFTER, HeaderValue::from_static("30"))
        .project::<HttpError>();
    let values: Vec<_> = resp.headers().get_all(RETRY_AFTER).iter().collect();
    assert_eq!(values, ["5"]);
}

#[r2e_core::test]
async fn repeated_hub_header_values_are_all_kept() {
    let resp = Rejection::unauthenticated()
        .header(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))
        .header(WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"r2e\""))
        .project::<HttpError>();
    let values: Vec<_> = resp.headers().get_all(WWW_AUTHENTICATE).iter().collect();
    assert_eq!(values, ["Bearer", "Basic realm=\"r2e\""]);
}

#[r2e_core::test]
async fn envelope_header_wins_over_every_repeated_hub_value() {
    let resp = Rejection::unauthenticated()
        .header(RETRY_AFTER, HeaderValue::from_static("1"))
        .header(RETRY_AFTER, HeaderValue::from_static("2"))
        .project::<Envelope>();
    let values: Vec<_> = resp.headers().get_all(RETRY_AFTER).iter().collect();
    assert_eq!(values, ["envelope"]);
}

// ── status remap ─────────────────────────────────────────────────────────

#[r2e_core::test]
async fn envelope_status_remap_is_applied_before_from() {
    let (status, body) = json_parts(Rejection::from(validation()).project::<Envelope>()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["status"], 422, "From<Rejection> saw the remapped status");
    assert_eq!(body["type"], "Validation");
}

#[r2e_core::test]
async fn carried_status_survives_when_the_envelope_does_not_remap() {
    let r = Rejection::with_status(RejectionKind::BadRequest, StatusCode::IM_A_TEAPOT, "tea");
    let (status, _) = json_parts(r.project::<Envelope>()).await;
    assert_eq!(status, StatusCode::IM_A_TEAPOT);

    let r = Rejection::with_status(RejectionKind::BadRequest, StatusCode::IM_A_TEAPOT, "tea");
    assert_eq!(
        r.project::<HttpError>().status(),
        StatusCode::IM_A_TEAPOT
    );
}

#[test]
fn projection_status_is_coherent_with_status_of() {
    // A `Validation` rejection is built the way the framework builds it —
    // with populated field-error `details` — since that is the branch where
    // `HttpError` rebuilds a dedicated variant.
    let sample = |kind: RejectionKind| match kind {
        RejectionKind::Validation => Rejection::from(validation()),
        kind => Rejection::new(kind, Cow::Borrowed("m")),
    };
    for kind in RejectionKind::ALL {
        if *kind == RejectionKind::Opaque {
            continue;
        }
        assert_eq!(
            sample(*kind).project::<HttpError>().status(),
            HttpError::status_of(*kind),
            "HttpError {kind:?}"
        );
        assert_eq!(
            sample(*kind).project::<Envelope>().status(),
            Envelope::status_of(*kind),
            "Envelope {kind:?}"
        );
        assert_eq!(
            sample(*kind).project::<Delegating>().status(),
            Delegating::status_of(*kind),
            "Delegating {kind:?}"
        );
    }
}

#[r2e_core::test]
async fn validation_remap_survives_an_envelope_delegating_to_http_error() {
    let mut report = garde::Report::new();
    report.append(garde::Path::new("email"), garde::Error::new("not an email"));
    let (status, body) = json_parts(Rejection::from(&report).project::<Delegating>()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "Validation failed");
    assert_eq!(body["details"][0]["field"], "email");
}

#[r2e_core::test]
async fn default_validation_body_is_byte_equal_to_0_4() {
    let (status, body) = parts(Rejection::from(validation()).project::<HttpError>()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        String::from_utf8_lossy(&body),
        r#"{"details":[{"code":"required","field":"name","message":"required"}],"error":"Validation failed"}"#
    );
}

// ── ErrorSchema for HttpError ────────────────────────────────────────────

#[test]
fn http_error_schema_describes_the_documented_bodies() {
    assert!(HttpError::opaque_passthrough());
    assert!(HttpError::extra_statuses().is_empty());
    let (name, schema) = HttpError::body_schema().unwrap();
    assert_eq!(name, "ErrorResponse");
    assert_eq!(schema["required"][0], "error");
    let (name, schema) = HttpError::body_schema_for(RejectionKind::Validation).unwrap();
    assert_eq!(name, "ValidationErrorResponse");
    assert_eq!(schema["properties"]["details"]["type"], "array");
    assert!(HttpError::body_schema_for(RejectionKind::NotFound).is_none());
}
