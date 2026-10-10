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
use r2e_core::di::meta::{
    MetaRegistry, RequestBodySchema, ResponseBodySchema, ResponseContent, RouteInfo,
};
use r2e_core::error::{ErrorSchema, Rejection, RejectionKind};
use r2e_core::http::extract::{FromRequest, Path, Query};
use r2e_core::http::response::{IntoHttpResponse, IntoResponse};
use r2e_core::http::{Form, Json, Request, Response, Sse, SseEvent, StatusCode};
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

#[derive(Deserialize, schemars::JsonSchema)]
struct Plain {
    #[allow(dead_code)]
    name: String,
}

#[derive(Deserialize, garde::Validate, schemars::JsonSchema)]
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

/// A reply served as JSON or as an SSE stream, documented through
/// `ResponseBodySchema`.
enum Reply {
    Json(Json<&'static str>),
}

impl IntoResponse for Reply {
    fn into_response(self) -> Response {
        match self {
            Reply::Json(j) => j.into_response(),
        }
    }
}

impl ResponseBodySchema for Reply {
    fn response_contents() -> Vec<ResponseContent> {
        vec![
            ResponseContent::json(Some((
                "Completion".to_string(),
                serde_json::json!({ "type": "object" }),
            ))),
            ResponseContent::event_stream(Some((
                "Chunk".to_string(),
                serde_json::json!({ "type": "object" }),
            ))),
        ]
    }
}

/// A response type without `ResponseBodySchema` → still unmapped.
struct OpaqueReply;

impl IntoResponse for OpaqueReply {
    fn into_response(self) -> Response {
        StatusCode::OK.into_response()
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

    /// Opaque success type, nameable error: still the route envelope.
    #[post("/envelope-opaque")]
    async fn envelope_opaque(
        &self,
        Json(_b): Json<Plain>,
    ) -> std::result::Result<impl IntoResponse, Envelope> {
        Ok("ok")
    }

    /// Redundant parentheses around the `Result` are peeled.
    #[post("/envelope-opaque-paren")]
    #[allow(unused_parens)]
    async fn envelope_opaque_paren(
        &self,
        Json(_b): Json<Plain>,
    ) -> (std::result::Result<impl IntoResponse, Envelope>) {
        Ok("ok")
    }

    /// Opaque everywhere: nothing to name, the application projection.
    #[get("/all-opaque")]
    async fn all_opaque(&self) -> impl IntoResponse {
        "ok"
    }

    /// A query-string form: only the deserialization can fail.
    #[get("/form-query")]
    async fn form_query(&self, Form(_q): Form<Plain>) -> &'static str {
        "ok"
    }

    /// A form body: media type, size, read, then deserialization (422).
    #[post("/form-body")]
    async fn form_body(&self, Form(_b): Form<Plain>) -> &'static str {
        "ok"
    }

    #[post("/csv")]
    async fn csv(&self, _b: CsvBody) -> &'static str {
        "ok"
    }

    #[post("/opaque")]
    async fn opaque(&self, _b: Opaque) -> &'static str {
        "ok"
    }

    #[post("/reply")]
    async fn reply(&self) -> Result<Reply, Envelope> {
        Ok(Reply::Json(Json("ok")))
    }

    #[post("/reply-plain")]
    async fn reply_plain(&self) -> Reply {
        Reply::Json(Json("ok"))
    }

    #[post("/opaque-reply")]
    async fn opaque_reply(&self) -> OpaqueReply {
        OpaqueReply
    }

    /// An `impl Trait` inside the return type is never probed.
    #[post("/sse")]
    async fn sse(&self) -> Sse<impl futures_core::Stream<Item = Result<SseEvent, Infallible>>> {
        Sse::new(r2e_core::rt::stream::empty())
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

/// Optional struct-level identity: a present but invalid token still fails.
#[controller(path = "/maybe")]
struct MaybeController {
    #[inject(identity)]
    #[allow(dead_code)]
    user: Option<Subject>,
}

#[routes]
impl MaybeController {
    #[get("/me")]
    async fn me(&self) -> &'static str {
        "ok"
    }
}

/// Streaming routes read the same request parameters as plain routes.
#[controller(path = "/streams")]
struct StreamsController {}

#[routes]
impl StreamsController {
    #[sse("/events")]
    async fn events(
        &self,
        Query(_q): Query<Validated>,
    ) -> impl futures_core::Stream<Item = Result<r2e_core::http::response::SseEvent, Infallible>>
    {
        r2e_core::rt::stream::empty()
    }
}

#[cfg(feature = "ws")]
#[controller(path = "/sockets")]
struct SocketsController {}

#[cfg(feature = "ws")]
#[routes]
impl SocketsController {
    #[ws("/socket")]
    async fn socket(&self, Query(_q): Query<Validated>, _ws: r2e_core::web::ws::WsStream) {}
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
fn identity_parameter_is_unauthenticated_required_or_optional() {
    assert!(has(
        &kinds!(OpenController, "/open/required"),
        RejectionKind::Unauthenticated
    ));
    // `Option<identity>` admits a missing token, not an invalid one: the
    // extraction still answers 401 on a present but bad token.
    assert_eq!(
        kinds!(OpenController, "/open/optional"),
        sorted(vec![RejectionKind::Unauthenticated, RejectionKind::Internal])
    );
}

#[test]
fn optional_struct_identity_is_unauthenticated() {
    assert_eq!(
        kinds!(MaybeController, "/maybe/me"),
        sorted(vec![RejectionKind::Unauthenticated, RejectionKind::Internal])
    );
}

#[test]
fn form_kinds_follow_the_method() {
    use RejectionKind::*;
    assert_eq!(
        kinds!(OpenController, "/open/form-query"),
        sorted(vec![InvalidForm, Internal])
    );
    assert_eq!(
        kinds!(OpenController, "/open/form-body"),
        sorted(vec![
            UnsupportedMediaType,
            PayloadTooLarge,
            BodyRead,
            InvalidBody,
            Internal
        ])
    );
}

#[test]
fn streaming_routes_infer_from_their_parameters() {
    use RejectionKind::*;
    let expected = sorted(vec![InvalidQuery, Validation, Internal]);
    assert_eq!(kinds!(StreamsController, "/streams/events"), expected);
    #[cfg(feature = "ws")]
    assert_eq!(kinds!(SocketsController, "/sockets/socket"), expected);
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
fn opaque_success_type_still_captures_the_envelope_schema() {
    let r = route!(OpenController, "/open/envelope-opaque");
    let schema = r
        .error_schema
        .expect("envelope captured from Result<impl Trait, Envelope>");
    assert_eq!(schema.type_name(), std::any::type_name::<Envelope>());
    assert_eq!(
        schema.body_schema().map(|(n, _)| n).as_deref(),
        Some("EnvelopeBody")
    );
    let paren = route!(OpenController, "/open/envelope-opaque-paren")
        .error_schema
        .expect("envelope captured from (Result<impl Trait, Envelope>)");
    assert_eq!(paren.type_name(), std::any::type_name::<Envelope>());
    assert!(route!(OpenController, "/open/all-opaque")
        .error_schema
        .is_none());
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
    let body = r.request_body.as_ref().expect("documented body");
    assert_eq!(body.content_type, "text/csv");
    assert!(body.required);
    assert_eq!(
        body.schema,
        Some(("Csv".to_string(), serde_json::json!({ "type": "string" })))
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
fn json_body_is_documented_through_the_framework_impl() {
    let r = route!(OpenController, "/open/json");
    let body = r.request_body.as_ref().expect("documented body");
    assert_eq!(body.content_type, "application/json");
    assert!(body.required);
    assert!(r.request_body_unmapped.is_none());
    // The JSON schema itself needs the `openapi` feature (schemars), which
    // feature unification may or may not enable for this target: when it is
    // present it is `Plain`'s schema.
    if let Some((name, _)) = &body.schema {
        assert_eq!(name, "Plain");
    }
}

#[test]
fn body_type_without_request_body_schema_is_undocumented() {
    let r = route!(OpenController, "/open/opaque");
    assert!(r.request_body.is_none());
    // Flagged for the OpenAPI boot warning, by readable type name.
    assert_eq!(r.request_body_unmapped.as_deref(), Some("Opaque"));
    assert_eq!(r.rejection_kinds, vec![RejectionKind::Internal]);
    assert!(route!(OpenController, "/open/csv")
        .request_body_unmapped
        .is_none());
    assert!(route!(OpenController, "/open/bare")
        .request_body_unmapped
        .is_none());
}

// ── Custom response types ──────────────────────────────────────────────────

fn content_types(r: &RouteInfo) -> Vec<&str> {
    r.response_contents
        .iter()
        .map(|c| c.content_type.as_str())
        .collect()
}

#[test]
fn response_body_schema_documents_every_media_type() {
    for path in ["/open/reply", "/open/reply-plain"] {
        let r = route!(OpenController, path);
        assert_eq!(
            content_types(&r),
            vec!["application/json", "text/event-stream"],
            "{path}"
        );
        assert_eq!(
            r.response_contents[0].schema.as_ref().map(|(n, _)| n.as_str()),
            Some("Completion")
        );
        assert!(r.response_unmapped.is_none(), "{path}");
    }
}

#[test]
fn response_type_without_response_body_schema_stays_unmapped() {
    let r = route!(OpenController, "/open/opaque-reply");
    assert!(r.response_contents.is_empty());
    assert!(r.response_unmapped.is_some());
}

#[test]
fn framework_return_types_are_documented_through_their_impls() {
    assert_eq!(
        content_types(&route!(OpenController, "/open/bare")),
        vec!["text/plain"]
    );
    assert_eq!(
        content_types(&route!(OpenController, "/open/sse")),
        vec!["text/event-stream"]
    );
    assert!(route!(OpenController, "/open/bare")
        .response_unmapped
        .is_none());
}

#[test]
fn impl_trait_returns_stay_unmapped() {
    let r = route!(OpenController, "/open/all-opaque");
    assert!(r.response_contents.is_empty());
    assert!(r.response_unmapped.is_some());
}
