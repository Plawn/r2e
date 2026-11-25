use http_body_util::BodyExt;
use r2e_core::http::{IntoResponse, StatusCode};
use r2e_core::HttpError;

async fn error_parts(err: HttpError) -> (StatusCode, serde_json::Value) {
    let resp = err.into_response();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

#[r2e_core::test]
async fn app_error_not_found_status() {
    let (status, body) = error_parts(HttpError::NotFound("resource missing".into())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "resource missing");
}

#[r2e_core::test]
async fn app_error_bad_request_status() {
    let (status, body) = error_parts(HttpError::BadRequest("invalid input".into())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid input");
}

#[r2e_core::test]
async fn app_error_unauthorized_status() {
    let (status, body) = error_parts(HttpError::Unauthorized("no token".into())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "no token");
}

#[r2e_core::test]
async fn app_error_forbidden_status() {
    let (status, body) = error_parts(HttpError::Forbidden("access denied".into())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "access denied");
}

#[r2e_core::test]
async fn app_error_internal_status() {
    let (status, body) = error_parts(HttpError::Internal("server broke".into())).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"], "server broke");
}

#[r2e_core::test]
async fn app_error_custom_status_and_body() {
    let custom_body = serde_json::json!({"detail": "teapot"});
    let (status, body) = error_parts(HttpError::Custom {
        status: StatusCode::IM_A_TEAPOT,
        body: custom_body.clone(),
    })
    .await;
    assert_eq!(status, StatusCode::IM_A_TEAPOT);
    assert_eq!(body, custom_body);
}

#[test]
fn app_error_display_formatting() {
    assert_eq!(HttpError::NotFound("x".into()).to_string(), "Not Found: x");
    assert_eq!(
        HttpError::Unauthorized("y".into()).to_string(),
        "Unauthorized: y"
    );
    assert_eq!(HttpError::Forbidden("z".into()).to_string(), "Forbidden: z");
    assert_eq!(
        HttpError::BadRequest("w".into()).to_string(),
        "Bad Request: w"
    );
    assert_eq!(
        HttpError::Internal("v".into()).to_string(),
        "Internal Error: v"
    );
}

#[test]
fn app_error_from_io_error() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    let app_err: HttpError = io_err.into();
    match app_err {
        HttpError::Internal(msg) => assert!(msg.contains("file missing")),
        other => panic!("expected Internal, got {other}"),
    }
}

#[derive(Debug)]
struct SentinelError(u32);

impl std::fmt::Display for SentinelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sentinel({})", self.0)
    }
}

impl std::error::Error for SentinelError {}

#[test]
fn with_source_clone_preserves_source_type() {
    use std::error::Error;
    use std::sync::Arc;

    let original = HttpError::WithSource {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "boom".into(),
        source: Arc::new(SentinelError(42)),
    };

    let cloned = original.clone();

    let src = <HttpError as Error>::source(&cloned).expect("cloned source missing");
    let downcast = src
        .downcast_ref::<SentinelError>()
        .expect("downcast should succeed after clone");
    assert_eq!(downcast.0, 42);
}

#[test]
fn from_status_picks_the_dedicated_variant() {
    use std::borrow::Cow;
    let cases: [(StatusCode, fn(&HttpError) -> bool); 5] = [
        (StatusCode::NOT_FOUND, |e| {
            matches!(e, HttpError::NotFound(_))
        }),
        (StatusCode::UNAUTHORIZED, |e| {
            matches!(e, HttpError::Unauthorized(_))
        }),
        (StatusCode::FORBIDDEN, |e| {
            matches!(e, HttpError::Forbidden(_))
        }),
        (StatusCode::BAD_REQUEST, |e| {
            matches!(e, HttpError::BadRequest(_))
        }),
        (StatusCode::INTERNAL_SERVER_ERROR, |e| {
            matches!(e, HttpError::Internal(_))
        }),
    ];
    for (status, is_variant) in cases {
        let err = HttpError::from_status(status, Cow::Borrowed("boom"));
        assert!(is_variant(&err), "{status}: {err:?}");
        assert_eq!(err.status(), status);
        assert_eq!(err.message(), Some("boom"));
    }

    let err = HttpError::from_status(StatusCode::CONFLICT, "dup");
    assert!(matches!(err, HttpError::Custom { .. }), "{err:?}");
    assert_eq!(err.status(), StatusCode::CONFLICT);
    assert_eq!(err.message(), None);
}

#[test]
fn status_and_message_of_every_shape() {
    let with_source = HttpError::WithSource {
        status: StatusCode::BAD_GATEWAY,
        message: "upstream".into(),
        source: std::sync::Arc::new(std::io::Error::other("io")),
    };
    assert_eq!(with_source.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(with_source.message(), Some("upstream"));

    assert_eq!(HttpError::not_found("nf").status(), StatusCode::NOT_FOUND);
    assert_eq!(HttpError::forbidden("no").message(), Some("no"));
    assert_eq!(
        HttpError::internal("oops").status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}
