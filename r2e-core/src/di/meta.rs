use serde::Serialize;
use serde_json::Value;

use crate::error::{ErrorSchemaInfo, RejectionKind};
use std::any::{Any, TypeId};
use std::collections::HashMap;

/// A generic, type-erased metadata registry.
///
/// Plugins register typed consumers via
/// [`AppBuilder::with_meta_consumer`](crate::builder::AppBuilder::with_meta_consumer),
/// and controllers push metadata into the registry via
/// [`Controller::register_meta`](crate::controller::Controller::register_meta).
///
/// Internally stores `Vec<M>` per type, keyed by `TypeId`.
#[derive(Default)]
pub struct MetaRegistry {
    inner: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl MetaRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a single metadata item into the registry.
    pub fn push<M: Any + Send + Sync>(&mut self, item: M) {
        self.entry::<M>().push(item);
    }

    /// Extend the registry with multiple metadata items.
    pub fn extend<M: Any + Send + Sync>(&mut self, items: impl IntoIterator<Item = M>) {
        self.entry::<M>().extend(items);
    }

    /// Take all metadata of a given type, leaving the slot empty.
    pub fn take<M: Any + Send + Sync>(&mut self) -> Vec<M> {
        self.inner
            .remove(&TypeId::of::<M>())
            .and_then(|boxed| boxed.downcast::<Vec<M>>().ok())
            .map(|v| *v)
            .unwrap_or_default()
    }

    /// Get a shared reference to all metadata of a given type.
    pub fn get<M: Any + Send + Sync>(&self) -> Option<&[M]> {
        self.inner
            .get(&TypeId::of::<M>())
            .and_then(|boxed| boxed.downcast_ref::<Vec<M>>())
            .map(|v| v.as_slice())
    }

    /// Get a shared reference to all metadata of a given type, or an empty slice.
    pub fn get_or_empty<M: Any + Send + Sync>(&self) -> &[M] {
        self.get::<M>().unwrap_or(&[])
    }

    /// Get a mutable reference to all metadata of a given type.
    ///
    /// Used to rewrite items a controller just pushed — e.g. prefixing
    /// [`RouteInfo::path`] when the controller is mounted under a feature
    /// module's `prefix`, so the published OpenAPI path is the mounted one.
    pub fn get_mut<M: Any + Send + Sync>(&mut self) -> Option<&mut Vec<M>> {
        self.inner
            .get_mut(&TypeId::of::<M>())
            .and_then(|boxed| boxed.downcast_mut::<Vec<M>>())
    }

    /// Get or create the `Vec<M>` entry for a given type.
    fn entry<M: Any + Send + Sync>(&mut self) -> &mut Vec<M> {
        self.inner
            .entry(TypeId::of::<M>())
            .or_insert_with(|| Box::new(Vec::<M>::new()))
            .downcast_mut::<Vec<M>>()
            .expect("MetaRegistry: type mismatch (should be impossible)")
    }
}

// ── Metadata types (moved from openapi.rs) ──────────────────────────────────

/// Metadata about a single route, collected at compile time.
#[derive(Debug, Clone, Serialize)]
pub struct RouteInfo {
    pub path: String,
    pub method: String,
    pub operation_id: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    /// The request body the route reads, documented through the body
    /// extractor's [`RequestBodySchema`] impl. `None` when the route reads no
    /// body, or its extractor does not implement the trait.
    pub request_body: Option<RequestBody>,
    /// Readable name of the body extractor type whose request body could
    /// not be mapped: a body-position parameter without a
    /// [`RequestBodySchema`] impl (including `Json<T>` when `T` lacks
    /// `schemars::JsonSchema` under the `openapi` feature). `None` when the
    /// body is documented or the route reads none.
    ///
    /// Set by the `#[routes]` macro; consumed by OpenAPI spec generation to
    /// emit a once-at-boot warning instead of silently dropping the body and
    /// its rejection kinds.
    pub request_body_unmapped: Option<String>,
    pub response_status: u16,
    /// The media types the **successful** response can be served as, from the
    /// return type's [`ResponseBodySchema`] impl (`Json<T>`, `String`,
    /// `Html<_>`, a custom enum, …). Empty for an intentional no-body return
    /// (`()`, `StatusCode`, `Redirect`, 204) and for an unmapped one.
    pub response_contents: Vec<ResponseContent>,
    /// The Rust return-type name of a **successful** response body that could
    /// not be mapped: an `impl Trait` return, or a concrete type without a
    /// [`ResponseBodySchema`] impl. `None` for mapped or body-less routes.
    ///
    /// Set by the `#[routes]` macro; consumed by OpenAPI spec generation to
    /// emit a once-at-boot warning naming the route and offending type instead
    /// of silently documenting the response without a body.
    pub response_unmapped: Option<String>,
    pub params: Vec<ParamInfo>,
    pub roles: Vec<String>,
    pub tag: Option<String>,
    pub deprecated: bool,
    /// Failure kinds the request pipeline can produce for this route before
    /// (or around) the handler: inferred by the `#[routes]` macro from the
    /// parameters (body extractor, `Path`/`Query`/`#[derive(Params)]`
    /// locations, garde validation), the identity (struct-level or required
    /// parameter → `Unauthenticated`), roles and guards (`Forbidden`, or
    /// `RateLimited` for a rate-limit guard), plus `Internal` always. The
    /// OpenAPI builder documents one error response per distinct
    /// `status_of(kind)` of the route's envelope.
    pub rejection_kinds: Vec<RejectionKind>,
    /// The route's own error envelope — the `E` of a `Result<T, E>` return
    /// type — when it declares one. `None` means the application projection
    /// (the `ErrorProjector` bean, else `HttpError`) documents the route.
    #[serde(skip)]
    pub error_schema: Option<ErrorSchemaInfo>,
}

/// Describes a multipart form type as a JSON Schema object for OpenAPI.
///
/// `#[derive(FromMultipart)]` generates this impl automatically. The routes
/// macro probes for it (autoref specialization) when a handler takes
/// `TypedMultipart<T>`, so a manual `FromMultipart` impl without it simply
/// yields a schema-less `multipart/form-data` body in the generated spec.
///
/// Lives here (not in the feature-gated `multipart` module) because the
/// routes macro emits the probe for any `TypedMultipart`-shaped parameter,
/// including in apps that never enable the `multipart` feature.
pub trait MultipartSchema {
    /// The JSON Schema describing the form:
    /// `{"type": "object", "properties": {...}, "required": [...]}`.
    /// File fields are modeled as `{"type": "string", "format": "binary"}`.
    fn multipart_schema() -> Value;

    /// Component name of the form schema (the type's name by default).
    fn schema_name() -> &'static str;
}

/// Describes a request-body extractor for OpenAPI.
///
/// The routes macro probes the handler's body parameter (its last extracted
/// parameter, read through `FromRequest`; `Option<..>` unwrapped) for this
/// trait by autoref specialization. The framework implements it for its own
/// extractors — `Json<T: JsonSchema>` (with the `openapi` feature), `Bytes`,
/// `String`, `Multipart`, `TypedMultipart<T: MultipartSchema>` — and a custom
/// extractor documents its media type, schema and failure kinds the same way.
/// Without an impl the route documents no request body.
///
/// ```rust,ignore
/// impl<T: JsonSchema> RequestBodySchema for OpenAiBody<T> {
///     fn content_type() -> &'static str { "application/json" }
///     fn body_schema() -> Option<(String, Value)> { Some(schema_of::<T>()) }
///     fn rejection_kinds() -> Vec<RejectionKind> {
///         vec![RejectionKind::MissingContentType, RejectionKind::InvalidBody]
///     }
/// }
/// ```
pub trait RequestBodySchema {
    /// Media type the extractor reads (`application/json`, …).
    fn content_type() -> &'static str;
    /// `(component name, JSON Schema)` of the body. `None` (the default)
    /// documents a string body for `text/*`, a binary string for
    /// `application/octet-stream` and a free-form object otherwise.
    fn body_schema() -> Option<(String, Value)> {
        None
    }
    /// Failure kinds the extractor can reject with; they are documented as
    /// error responses through the route's envelope.
    fn rejection_kinds() -> Vec<RejectionKind>;
}

/// One media type a successful response can be served as.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseContent {
    /// Media type (`application/json`, `text/event-stream`, `text/plain`, …).
    pub content_type: String,
    /// `(component name, JSON Schema)` of the body under this media type.
    /// `None` documents a string for `text/*` media types and a free-form
    /// value otherwise.
    pub schema: Option<(String, Value)>,
}

impl ResponseContent {
    /// A media type carrying the given `(component name, schema)`.
    pub fn new(content_type: impl Into<String>, schema: Option<(String, Value)>) -> Self {
        Self {
            content_type: content_type.into(),
            schema,
        }
    }

    /// `application/json` with the given schema.
    pub fn json(schema: Option<(String, Value)>) -> Self {
        Self::new("application/json", schema)
    }

    /// `text/event-stream`; `schema` describes one event's `data` payload.
    pub fn event_stream(schema: Option<(String, Value)>) -> Self {
        Self::new("text/event-stream", schema)
    }

    /// `text/plain`, documented as a string.
    pub fn text() -> Self {
        Self::new("text/plain", None)
    }

    /// `text/html`, documented as a string.
    pub fn html() -> Self {
        Self::new("text/html", None)
    }

    /// `application/octet-stream`, documented as a binary string.
    pub fn binary() -> Self {
        Self::new(OCTET_STREAM, None)
    }
}

/// The `application/octet-stream` media type (binary bodies).
pub const OCTET_STREAM: &str = "application/octet-stream";

/// Describes a response type for OpenAPI.
///
/// The routes macro probes the handler's concrete return type (the `T` of a
/// `Result<T, E>`, or the type named by `#[returns(T)]` for an `impl Trait`
/// return) for this trait by autoref specialization. The framework implements
/// it for its own response types — `Json<T: JsonSchema>` (with the `openapi`
/// feature), `String`/`&str`, `Html<_>`, `Bytes`/`Vec<u8>`, the no-body
/// `()`/`StatusCode`/`Redirect`, and `(StatusCode, T)`/`(HeaderMap, T)`
/// tuples delegating to `T`. A custom type lists every media type it can be
/// served as, so a handler answering JSON or an SSE stream depending on the
/// request documents both under its success status. Without an impl the route
/// documents no body and spec generation warns about it.
///
/// ```rust,ignore
/// pub enum ChatReply {
///     Json(Json<Completion>),
///     Stream(Sse<BoxStream<'static, Result<Event, Infallible>>>),
/// }
///
/// impl ResponseBodySchema for ChatReply {
///     fn response_contents() -> Vec<ResponseContent> {
///         vec![
///             ResponseContent::json(Some(schema_of::<Completion>())),
///             ResponseContent::event_stream(Some(schema_of::<Chunk>())),
///         ]
///     }
/// }
/// ```
pub trait ResponseBodySchema {
    /// Every media type the response can be served as, in documentation
    /// order. An empty list documents no body.
    fn response_contents() -> Vec<ResponseContent>;
}

/// What the routes macro's `RequestBodySchema` probe yields for the body
/// extractor. Generated code only.
#[doc(hidden)]
pub struct __BodyProbeResult {
    pub content_type: &'static str,
    pub schema: Option<(String, Value)>,
    pub rejection_kinds: Vec<RejectionKind>,
}

/// The request body a route reads, as documented by its extractor's
/// [`RequestBodySchema`] impl.
#[derive(Debug, Clone, Serialize)]
pub struct RequestBody {
    /// Media type (`application/json`, `multipart/form-data`, …).
    pub content_type: String,
    /// `(component name, JSON Schema)` of the body. `None` documents a
    /// free-form object (raw `Multipart`) or a string / binary body for
    /// `text/*` / `application/octet-stream`.
    pub schema: Option<(String, Value)>,
    /// `false` when the extractor is `Option<..>`-wrapped.
    pub required: bool,
}

/// `(component name, JSON Schema)` of `T` — the pair
/// [`RequestBodySchema::body_schema`], [`ResponseContent::json`] and
/// [`ErrorSchema::body_schema`](crate::ErrorSchema) take. `$defs` are promoted
/// to `components/schemas` by the spec builder.
#[cfg(feature = "openapi")]
pub fn schema_of<T: schemars::JsonSchema + ?Sized>() -> (String, Value) {
    let schema = schemars::SchemaGenerator::default().into_root_schema_for::<T>();
    (T::schema_name().into_owned(), Value::from(schema))
}

/// Metadata about a route parameter.
#[derive(Debug, Clone, Serialize)]
pub struct ParamInfo {
    pub name: String,
    pub location: ParamLocation,
    pub param_type: String,
    pub required: bool,
}

/// Where a parameter is located in the HTTP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
}
