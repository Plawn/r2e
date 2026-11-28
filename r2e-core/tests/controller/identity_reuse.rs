//! A route method's `#[inject(identity)]` parameter reuses the controller's
//! struct-level identity when both are the same `Clone` type: the extractor
//! runs once per request, not once per declaration (task #1500).

use r2e_core::http::extract::FromRequestParts;
use r2e_core::http::response::{IntoResponse, Response};
use r2e_core::http::{Body, StatusCode};
use r2e_core::prelude::*;
use r2e_core::web::extract::OptionalFromRequestPartsVia;
use r2e_core::Identity;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::fixtures::req;

/// Counts every run of an identity extractor of this module.
#[derive(Clone, Default)]
struct Extractions(Arc<AtomicUsize>);

impl Extractions {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

fn user_header(parts: &r2e_core::http::header::Parts) -> Option<String> {
    parts
        .headers
        .get("x-user")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

fn record<S: r2e_core::type_list::BeanLookup>(state: &S) {
    state
        .bean_ref::<Extractions>()
        .expect("Extractions must be provided")
        .0
        .fetch_add(1, Ordering::SeqCst);
}

macro_rules! counted_identity {
    ($(#[$meta:meta])* $name:ident, $via:ident) => {
        $(#[$meta])*
        struct $name(String);

        impl Identity for $name {
            fn sub(&self) -> &str {
                &self.0
            }
        }

        impl<S: Send + Sync + r2e_core::type_list::BeanLookup> FromRequestParts<S> for $name {
            type Rejection = Response;

            async fn from_request_parts(
                parts: &mut r2e_core::http::header::Parts,
                state: &S,
            ) -> Result<Self, Self::Rejection> {
                record(state);
                user_header(parts)
                    .map($name)
                    .ok_or_else(|| StatusCode::UNAUTHORIZED.into_response())
            }
        }

        struct $via;

        impl<S: Send + Sync + r2e_core::type_list::BeanLookup>
            OptionalFromRequestPartsVia<S, $via> for $name
        {
            type Rejection = Response;

            async fn from_request_parts_via(
                parts: &mut r2e_core::http::header::Parts,
                state: &S,
            ) -> Result<Option<Self>, Self::Rejection> {
                record(state);
                Ok(user_header(parts).map($name))
            }
        }
    };
}

counted_identity!(
    /// The identity under test: `Clone`, so a parameter can reuse it.
    #[derive(Clone)]
    Caller,
    CallerViaOpt
);
counted_identity!(
    /// Another `Clone` identity type: never interchangeable with `Caller`.
    #[derive(Clone)]
    Other,
    OtherViaOpt
);
counted_identity!(
    /// Not `Clone`: cannot be reused, each declaration extracts.
    Unique,
    UniqueViaOpt
);

// ── Required struct-level identity ──────────────────────────────────────────

#[controller(path = "/required")]
struct RequiredController {
    #[inject(identity)]
    caller: Caller,
}

#[routes]
impl RequiredController {
    #[get("/field")]
    async fn field(&self) -> String {
        self.caller.0.clone()
    }

    #[get("/param")]
    async fn param(&self, #[inject(identity)] caller: Caller) -> String {
        format!("{}+{}", self.caller.0, caller.0)
    }

    #[get("/optional-param")]
    async fn optional_param(&self, #[inject(identity)] caller: Option<Caller>) -> String {
        format!("{}+{:?}", self.caller.0, caller.map(|c| c.0))
    }

    #[get("/other-param")]
    async fn other_param(&self, #[inject(identity)] other: Other) -> String {
        format!("{}+{}", self.caller.0, other.0)
    }

    #[sse("/events")]
    async fn events(
        &self,
        #[inject(identity)] caller: Caller,
    ) -> impl futures_core::Stream<Item = Result<SseEvent, std::convert::Infallible>> {
        futures_util::stream::iter(vec![Ok(SseEvent::default().data(caller.0))])
    }
}

// ── Optional struct-level identity ──────────────────────────────────────────

#[controller(path = "/optional")]
struct OptionalController {
    #[inject(identity)]
    caller: Option<Caller>,
}

#[routes]
impl OptionalController {
    #[get("/param")]
    async fn param(&self, #[inject(identity)] caller: Caller) -> String {
        format!("{:?}+{}", self.caller.as_ref().map(|c| &c.0), caller.0)
    }

    #[get("/optional-param")]
    async fn optional_param(&self, #[inject(identity)] caller: Option<Caller>) -> String {
        format!(
            "{:?}+{:?}",
            self.caller.as_ref().map(|c| &c.0),
            caller.map(|c| c.0)
        )
    }
}

// ── Non-`Clone` identity: no reuse ──────────────────────────────────────────

#[controller(path = "/unique")]
struct UniqueController {
    #[inject(identity)]
    unique: Unique,
}

#[routes]
impl UniqueController {
    #[get("/param")]
    async fn param(&self, #[inject(identity)] unique: Unique) -> String {
        format!("{}+{}", self.unique.0, unique.0)
    }
}

async fn app() -> (r2e_core::http::Routes, Extractions) {
    let extractions = Extractions::default();
    let router = r2e_core::AppBuilder::new()
        .provide(extractions.clone())
        .build_state()
        .await
        .register_controller::<RequiredController>()
        .register_controller::<OptionalController>()
        .register_controller::<UniqueController>()
        .build();
    (router, extractions)
}

#[r2e_core::test]
async fn struct_identity_alone_extracts_once() {
    let (router, extractions) = app().await;
    let (status, body) = req(router, "/required/field", Some("zoe"), None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "zoe"));
    assert_eq!(extractions.count(), 1);
}

#[r2e_core::test]
async fn parameter_reuses_the_struct_identity() {
    let (router, extractions) = app().await;
    let (status, body) = req(router, "/required/param", Some("zoe"), None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "zoe+zoe"));
    assert_eq!(extractions.count(), 1);
}

#[r2e_core::test]
async fn optional_parameter_reuses_the_required_struct_identity() {
    let (router, extractions) = app().await;
    let (status, body) = req(router, "/required/optional-param", Some("zoe"), None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "zoe+Some(\"zoe\")"));
    assert_eq!(extractions.count(), 1);
}

#[r2e_core::test]
async fn required_parameter_reuses_a_present_optional_struct_identity() {
    let (router, extractions) = app().await;
    let (status, body) = req(router, "/optional/param", Some("zoe"), None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "Some(\"zoe\")+zoe"));
    assert_eq!(extractions.count(), 1);
}

/// An absent optional struct-level identity cannot satisfy a required
/// parameter: the parameter extracts and answers with its own rejection.
#[r2e_core::test]
async fn required_parameter_still_rejects_when_the_optional_struct_identity_is_absent() {
    let (router, extractions) = app().await;
    let (status, _) = req(router, "/optional/param", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(extractions.count(), 2);
}

#[r2e_core::test]
async fn optional_parameter_reuses_an_optional_struct_identity() {
    let (router, extractions) = app().await;
    let (status, body) = req(router.clone(), "/optional/optional-param", Some("zoe"), None).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "Some(\"zoe\")+Some(\"zoe\")")
    );
    assert_eq!(extractions.count(), 1);

    let (status, body) = req(router, "/optional/optional-param", None, None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "None+None"));
    assert_eq!(extractions.count(), 2);
}

#[r2e_core::test]
async fn parameter_of_another_identity_type_extracts_on_its_own() {
    let (router, extractions) = app().await;
    let (status, body) = req(router, "/required/other-param", Some("zoe"), None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "zoe+zoe"));
    assert_eq!(extractions.count(), 2);
}

#[r2e_core::test]
async fn non_clone_identity_is_extracted_per_declaration() {
    let (router, extractions) = app().await;
    let (status, body) = req(router, "/unique/param", Some("zoe"), None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "zoe+zoe"));
    assert_eq!(extractions.count(), 2);
}

#[r2e_core::test]
async fn sse_parameter_reuses_the_struct_identity() {
    let (router, extractions) = app().await;
    let (status, body) =
        crate::support::send(router, "GET", "/required/events", &[("x-user", "zoe")], Body::empty())
            .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("data: zoe"), "{body}");
    assert_eq!(extractions.count(), 1);
}
