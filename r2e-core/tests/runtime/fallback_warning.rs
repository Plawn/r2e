//! The boot warning for a hand-written router fallback that takes unmatched
//! requests out of the application's error envelope (#1501).

use r2e_core::error::{ErrorSchema, Rejection, RejectionKind};
use r2e_core::http::response::IntoHttpResponse;
use r2e_core::http::{Response, Routes, StatusCode};
use r2e_core::AppBuilder;

use crate::shutdown_budget::capturing;

const WARNING: &str = "outside the application's error envelope";

#[derive(Debug)]
struct Wire(StatusCode);

impl From<Rejection> for Wire {
    fn from(r: Rejection) -> Self {
        Self(r.status)
    }
}

impl IntoHttpResponse for Wire {
    fn into_http_response(self) -> Response {
        use r2e_core::http::response::IntoResponse;
        self.0.into_response()
    }
}

r2e_core::http::impl_into_response!(Wire);

impl ErrorSchema for Wire {
    fn status_of(kind: RejectionKind) -> StatusCode {
        kind.default_status()
    }

    fn body_schema() -> Option<(String, serde_json::Value)> {
        None
    }
}

fn with_fallback<S: Clone + Send + Sync + 'static>() -> Routes<S> {
    Routes::new().fallback(|_, _| async { "mine" })
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_raw_fallback_next_to_an_error_projection_is_reported_at_boot() {
    let (captured, subscriber) = capturing();
    let builder = AppBuilder::new()
        .error_projection::<Wire>()
        .build_state()
        .await
        .merge_router(with_fallback());
    tracing::subscriber::with_default(subscriber, || {
        let _ = builder.build();
    });
    assert!(captured.contains(WARNING), "{}", captured.dump());
    assert!(
        captured.contains(file!()),
        "the warning names the registration: {}",
        captured.dump()
    );
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_raw_fallback_without_an_error_projection_is_silent() {
    let (captured, subscriber) = capturing();
    let builder = AppBuilder::new()
        .build_state()
        .await
        .merge_router(with_fallback());
    tracing::subscriber::with_default(subscriber, || {
        let _ = builder.build();
    });
    assert!(!captured.contains(WARNING), "{}", captured.dump());
}

#[r2e_core::test(flavor = "current_thread")]
async fn a_nested_fallback_is_silent() {
    let (captured, subscriber) = capturing();
    let builder = AppBuilder::new()
        .error_projection::<Wire>()
        .build_state()
        .await
        .merge_router(Routes::new().nest("/legacy", with_fallback()));
    tracing::subscriber::with_default(subscriber, || {
        let _ = builder.build();
    });
    assert!(!captured.contains(WARNING), "{}", captured.dump());
}
