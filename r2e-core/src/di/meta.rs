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
    pub request_body_type: Option<String>,
    pub request_body_schema: Option<Value>,
    /// Request body media type. `None` means `application/json`.
    pub request_body_content_type: Option<String>,
    pub request_body_required: bool,
    pub response_type: Option<String>,
    pub response_schema: Option<Value>,
    pub response_status: u16,
    /// The Rust return-type name of a **successful** response body that could
    /// not be auto-mapped to an OpenAPI schema (an `impl Trait` return, or a
    /// concrete type that is not `Json<T>`), when that body is non-trivial
    /// (i.e. not an intentional no-body return such as `()`, `StatusCode`, or
    /// `String`). `None` for mapped or intentionally body-less routes.
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
}

/// Describes a **custom request-body extractor** for OpenAPI.
///
/// The routes macro recognises `Json<T>`, `TypedMultipart<T>` and raw
/// `Multipart` by name. Any other type in the body position (the handler's
/// last extracted parameter, read through `FromRequest`) is probed for this
/// trait (autoref specialization): an implementation documents the body's
/// media type, schema and failure kinds; without one the route documents no
/// request body.
///
/// ```rust,ignore
/// impl<T: JsonSchema> RequestBodySchema for OpenAiBody<T> {
///     fn content_type() -> &'static str { "application/json" }
///     fn body_schema() -> Option<(String, Value)> {
///         Some((T::schema_name().into(), serde_json::to_value(schema_for!(T)).unwrap()))
///     }
///     fn rejection_kinds() -> Vec<RejectionKind> {
///         vec![RejectionKind::MissingContentType, RejectionKind::InvalidBody]
///     }
/// }
/// ```
pub trait RequestBodySchema {
    /// Media type the extractor reads (`application/json`, …).
    fn content_type() -> &'static str;
    /// `(component name, JSON Schema)` of the body. `None` documents a
    /// free-form object under [`content_type`](Self::content_type).
    fn body_schema() -> Option<(String, Value)>;
    /// Failure kinds the extractor can reject with; they are documented as
    /// error responses through the route's envelope.
    fn rejection_kinds() -> Vec<RejectionKind>;
}

/// What the routes macro's `RequestBodySchema` probe yields for a custom
/// body extractor. Generated code only.
#[doc(hidden)]
pub struct __BodyProbeResult {
    pub content_type: &'static str,
    pub schema: Option<(String, Value)>,
    pub rejection_kinds: Vec<RejectionKind>,
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
