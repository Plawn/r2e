//! Error metadata on `RouteInfo` (#1072 P2): the rejection kinds a route can
//! fail with, inferred by the routes macro, and the envelope schema captured
//! from a `Result<T, E>` return type.
//!
//! The inference is the macro's reading of the handler signature and
//! decorators (body extractor, `Path` / `Query` / `#[derive(Params)]`
//! locations, identity, roles/guards, garde) plus `Internal` for the handler
//! itself. The OpenAPI builder turns every kind into a response at the
//! envelope's `status_of(kind)`.

use std::convert::Infallible;

use r2e_core::controller::Controller;
use r2e_core::di::meta::{MetaRegistry, RequestBodySchema, RouteInfo};
use r2e_core::error::{ErrorSchema, Rejection, RejectionKind};
use r2e_core::http::extract::{FromRequest, Path, Query};
use r2e_core::http::response::{IntoHttpResponse, IntoResponse};
use r2e_core::http::{Json, Request, Response, StatusCode};
use r2e_core::type_list::HNil;
use r2e_core::{Guard, GuardContext, Identity, PreAuthGuard, SelfBuilt};
use r2e_macros::{controller, routes, Params};
use serde::Deserialize;

use crate::fixtures::Subject;

// ── Fixtures ───────────────────────────────────────────────────────────────

/// An envelope with a body schema and a remapped `InvalidBody`.
#[derive(Debug)]
struct Envelope(Rejection);

impl From<Rejection> for Envelope {
    fn from(r: Rejection) -> Self {
        Self(r)
    }
}

impl IntoHttpResponse for Envelope {
    fn into_http_response(self) -> Response {
        (self.0.status, self.0.message.into_owned()).into_response()
    }
}

r2e_core::http::impl_into_response!(Envelope);

impl ErrorSchema for Envelope {
    fn status_of(kind: RejectionKind) -> StatusCode {
        match kind {
            RejectionKind::InvalidBody => StatusCode::BAD_REQUEST,
            k => k.default_status(),
        }
    }

    fn body_schema() -> Option<(String, serde_json::Value)> {
        Some((
            "EnvelopeBody".to_string(),
            serde_json::json!({ "type": "object" }),
        ))
    }
}

/// Post-auth guard with an unrelated name → `Forbidden`.
struct Gate;
impl SelfBuilt for Gate {}
impl<I: Identity> Guard<I> for Gate {
    async fn check(&self, _ctx: &GuardContext<'_, I>) -> Result<(), Rejection> {
        Ok(())
    }
}

/// A guard whose spec type is named like the rate-limit family → `RateLimited`.
struct TestRateLimit;
impl SelfBuilt for TestRateLimit {}
impl<I: Identity> Guard<I> for TestRateLimit {
    async fn check(&self, _ctx: &GuardContext<'_, I>) -> Result<(), Rejection> {
        Ok(())
    }
}

/// Pre-auth guard named like the rate-limit family → `RateLimited`.
struct PreRateLimit;
impl SelfBuilt for PreRateLimit {}
impl PreAuthGuard for PreRateLimit {
    async fn check(
        &self,
        _ctx: &r2e_core::decorators::guards::PreAuthGuardContext<'_>,
    ) -> Result<(), Rejection> {
        Ok(())
    }
}

#[derive(Deserialize)]
struct Plain {
    #[allow(dead_code)]
    name: String,
}

#[derive(Deserialize, garde::Validate)]
struct Validated {
    #[garde(length(min = 1))]
    #[allow(dead_code)]
    name: String,
}

#[derive(Params, Deserialize)]
struct Filter {
    #[allow(dead_code)]
    q: Option<String>,
}

/// A custom body extractor that documents itself through `RequestBodySchema`.
struct CsvBody(#[allow(dead_code)] String);

impl<S: Send + Sync> FromRequest<S> for CsvBody {
    type Rejection = Response;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let bytes = r2e_core::http::Bytes::from_request(req, &())
            .await
            .map_err(IntoResponse::into_response)?;
        Ok(Self(String::from_utf8_lossy(&bytes).into_owned()))
    }
}

impl RequestBodySchema for CsvBody {
    fn content_type() -> &'static str {
        "text/csv"
    }

    fn body_schema() -> Option<(String, serde_json::Value)> {
        Some(("Csv".to_string(), serde_json::json!({ "type": "string" })))
    }

    fn rejection_kinds() -> Vec<RejectionKind> {
        vec![RejectionKind::UnsupportedMediaType, RejectionKind::MalformedBody]
    }
}

/// A body-position type without `RequestBodySchema` → no request body documented.
struct Opaque;

impl<S: Send + Sync> FromRequest<S> for Opaque {
    type Rejection = Infallible;

    async fn from_request(_req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self)
    }
}

// ── Controllers ────────────────────────────────────────────────────────────

/// No struct identity: every auth kind comes from the route.
#[controller(path = "/open")]
struct OpenController {}

#[routes]
impl OpenController {
    #[get("/bare")]
    async fn bare(&self) -> &'static str {
        "ok"
    }

    #[post("/json")]
    async fn json(&self, Json(_b): Json<Plain>) -> &'static str {
        "ok"
    }

    #[post("/validated")]
    async fn validated(&self, Json(_b): Json<Validated>) -> &'static str {
        "ok"
    }

    #[get("/path/{id}")]
    async fn path(&self, Path(_id): Path<u32>) -> &'static str {
        "ok"
    }

    #[get("/query")]
    async fn query(&self, Query(_q): Query<Plain>) -> &'static str {
        "ok"
    }

    #[get("/params")]
    async fn params(&self, _f: Filter) -> &'static str {
        "ok"
    }

    #[get("/required")]
    async fn required(&self, #[inject(identity)] _user: Subject) -> &'static str {
        "ok"
    }

    #[get("/optional")]
    async fn optional(&self, #[inject(identity)] _user: Option<Subject>) -> &'static str {
        "ok"
    }

    #[get("/guarded")]
    #[guard(Gate)]
    async fn guarded(&self, #[inject(identity)] _user: Subject) -> &'static str {
        "ok"
    }

    #[get("/limited")]
    #[guard(TestRateLimit)]
    async fn limited(&self, #[inject(identity)] _user: Subject) -> &'static str {
        "ok"
    }

    #[get("/pre-limited")]
    #[pre_guard(PreRateLimit)]
    async fn pre_limited(&self) -> &'static str {
        "ok"
    }

    #[post("/envelope")]
    async fn envelope(&self, Json(_b): Json<Plain>) -> Result<&'static str, Envelope> {
        Ok("ok")
    }

    #[post("/csv")]
    async fn csv(&self, _b: CsvBody) -> &'static str {
        "ok"
    }

    #[post("/opaque")]
    async fn opaque(&self, _b: Opaque) -> &'static str {
        "ok"
    }
}

/// Struct-level identity: `Unauthenticated` everywhere but `#[anonymous]`.
#[controller(path = "/secured")]
struct SecuredController {
    #[inject(identity)]
    #[allow(dead_code)]
    user: Subject,
}

#[routes]
impl SecuredController {
    #[get("/me")]
    async fn me(&self) -> &'static str {
        "ok"
    }

    #[get("/public")]
    #[anonymous]
    async fn public(&self) -> &'static str {
        "ok"
    }

    #[sse("/events")]
    async fn events(
        &self,
    ) -> impl futures_core::Stream<Item = Result<r2e_core::http::response::SseEvent, Infallible>>
    {
        r2e_core::rt::stream::empty()
    }
}

/// Controller-level guard: folded into every non-anonymous route.
#[controller(path = "/gated")]
struct GatedController {
    #[inject(identity)]
    #[allow(dead_code)]
    user: Subject,
}

#[routes]
#[guard(Gate)]
impl GatedController {
    #[get("/in")]
    async fn inside(&self) -> &'static str {
        "ok"
    }

    #[get("/out")]
    #[anonymous]
    async fn outside(&self) -> &'static str {
        "ok"
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// The routes a controller registers, read back from a bare registry (the
/// pattern of `streaming_meta`).
macro_rules! routes_of {
    ($c:ty) => {{
        let mut registry = MetaRegistry::new();
        <$c as Controller<HNil, _>>::register_meta(&mut registry);
        registry.take::<RouteInfo>()
    }};
}

fn find(routes: Vec<RouteInfo>, path: &str) -> RouteInfo {
    routes
        .iter()
        .find(|r| r.path == path)
        .unwrap_or_else(|| {
            panic!(
                "no route {path} in {:?}",
                routes.iter().map(|r| r.path.as_str()).collect::<Vec<_>>()
            )
        })
        .clone()
}

macro_rules! route {
    ($c:ty, $path:expr) => {
        find(routes_of!($c), $path)
    };
}

macro_rules! kinds {
    ($c:ty, $path:expr) => {
        sorted(route!($c, $path).rejection_kinds)
    };
}

fn sorted(mut kinds: Vec<RejectionKind>) -> Vec<RejectionKind> {
    kinds.sort_by_key(|k| format!("{k:?}"));
    kinds
}

fn has(kinds: &[RejectionKind], kind: RejectionKind) -> bool {
    kinds.contains(&kind)
}

// ── Kinds ──────────────────────────────────────────────────────────────────

#[test]
fn bare_route_can_only_fail_internally() {
    assert_eq!(kinds!(OpenController, "/open/bare"), vec![RejectionKind::Internal]);
}

#[test]
fn json_body_kinds() {
    use RejectionKind::*;
    assert_eq!(
        kinds!(OpenController, "/open/json"),
        sorted(vec![
            MissingContentType,
            PayloadTooLarge,
            BodyRead,
            MalformedBody,
            InvalidBody,
            Internal
        ])
    );
}

#[test]
fn garde_body_adds_validation() {
    let k = kinds!(OpenController, "/open/validated");
    assert!(has(&k, RejectionKind::Validation), "{k:?}");
    assert!(has(&k, RejectionKind::InvalidBody), "{k:?}");
    // A body type without `garde::Validate` never documents a validation error.
    assert!(!has(
        &kinds!(OpenController, "/open/json"),
        RejectionKind::Validation
    ));
}

#[test]
fn path_and_query_extractors() {
    assert_eq!(
        kinds!(OpenController, "/open/path/{id}"),
        sorted(vec![RejectionKind::InvalidPath, RejectionKind::Internal])
    );
    assert_eq!(
        kinds!(OpenController, "/open/query"),
        sorted(vec![RejectionKind::InvalidQuery, RejectionKind::Internal])
    );
}

#[test]
fn derive_params_locations_are_read_at_runtime() {
    // `Filter` is a `#[derive(Params)]` DTO: the macro cannot see its field
    // locations, the generated metadata reads them from `ParamInfo`.
    assert_eq!(
        kinds!(OpenController, "/open/params"),
        sorted(vec![RejectionKind::InvalidQuery, RejectionKind::Internal])
    );
}

#[test]
fn required_identity_parameter_is_unauthenticated_optional_is_not() {
    assert!(has(
        &kinds!(OpenController, "/open/required"),
        RejectionKind::Unauthenticated
    ));
    assert_eq!(
        kinds!(OpenController, "/open/optional"),
        vec![RejectionKind::Internal]
    );
}

#[test]
fn guards_are_forbidden_unless_rate_limit() {
    let guarded = kinds!(OpenController, "/open/guarded");
    assert!(has(&guarded, RejectionKind::Forbidden), "{guarded:?}");
    assert!(!has(&guarded, RejectionKind::RateLimited), "{guarded:?}");

    let limited = kinds!(OpenController, "/open/limited");
    assert!(has(&limited, RejectionKind::RateLimited), "{limited:?}");
    assert!(!has(&limited, RejectionKind::Forbidden), "{limited:?}");

    assert_eq!(
        kinds!(OpenController, "/open/pre-limited"),
        sorted(vec![RejectionKind::RateLimited, RejectionKind::Internal])
    );
}

#[test]
fn struct_identity_is_unauthenticated_except_anonymous() {
    assert_eq!(
        kinds!(SecuredController, "/secured/me"),
        sorted(vec![RejectionKind::Unauthenticated, RejectionKind::Internal])
    );
    assert_eq!(
        kinds!(SecuredController, "/secured/public"),
        vec![RejectionKind::Internal]
    );
    // Streaming routes carry the same inference.
    assert_eq!(
        kinds!(SecuredController, "/secured/events"),
        sorted(vec![RejectionKind::Unauthenticated, RejectionKind::Internal])
    );
}

#[test]
fn controller_guard_folds_into_non_anonymous_routes_only() {
    assert_eq!(
        kinds!(GatedController, "/gated/in"),
        sorted(vec![
            RejectionKind::Unauthenticated,
            RejectionKind::Forbidden,
            RejectionKind::Internal
        ])
    );
    assert_eq!(
        kinds!(GatedController, "/gated/out"),
        vec![RejectionKind::Internal]
    );
}

// ── Envelope schema ────────────────────────────────────────────────────────

#[test]
fn result_envelope_return_type_captures_its_schema() {
    let r = route!(OpenController, "/open/envelope");
    let schema = r.error_schema.expect("envelope captured from Result<_, Envelope>");
    assert_eq!(schema.type_name(), std::any::type_name::<Envelope>());
    assert_eq!(
        schema.status_of(RejectionKind::InvalidBody),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        schema.body_schema().map(|(n, _)| n).as_deref(),
        Some("EnvelopeBody")
    );
}

#[test]
fn infallible_route_documents_through_the_application_projection() {
    assert!(route!(OpenController, "/open/bare").error_schema.is_none());
    assert!(route!(SecuredController, "/secured/events")
        .error_schema
        .is_none());
}

// ── Custom body extractors ─────────────────────────────────────────────────

#[test]
fn request_body_schema_extractor_is_documented() {
    let r = route!(OpenController, "/open/csv");
    assert_eq!(r.request_body_content_type.as_deref(), Some("text/csv"));
    assert_eq!(r.request_body_type.as_deref(), Some("Csv"));
    assert_eq!(
        r.request_body_schema,
        Some(serde_json::json!({ "type": "string" }))
    );
    let k = sorted(r.rejection_kinds);
    assert_eq!(
        k,
        sorted(vec![
            RejectionKind::UnsupportedMediaType,
            RejectionKind::MalformedBody,
            RejectionKind::Internal
        ])
    );
}

#[test]
fn body_type_without_request_body_schema_is_undocumented() {
    let r = route!(OpenController, "/open/opaque");
    assert!(r.request_body_content_type.is_none());
    assert!(r.request_body_type.is_none());
    assert_eq!(r.rejection_kinds, vec![RejectionKind::Internal]);
}
