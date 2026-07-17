use crate::route::HttpMethod;

pub struct InjectedField {
    pub name: syn::Ident,
    /// The declared field type. Used by `ContextConstruct` codegen
    /// (`ctx.get::<Ty>()`) and the controller's `Deps` list.
    pub ty: syn::Type,
}

pub struct IdentityField {
    pub name: syn::Ident,
    /// The type as declared on the struct (may be `Option<T>`). Used for the
    /// request-data field, the façade field, and `FromRequestParts` extraction.
    pub ty: syn::Type,
    /// The identity type unwrapped from `Option<T>`. Used for `IdentityType` and
    /// `guard_identity` (guards see `Option<&T>`, never `Option<&Option<T>>`).
    pub inner_ty: syn::Type,
    /// Whether the field was declared as `Option<T>`.
    pub is_optional: bool,
}

/// A `#[inject(request)]` field: any type implementing `FromRequestParts<State>`,
/// extracted per request and moved onto the generated request façade. Unlike
/// identity fields, request fields are not visible to guards. Multiple are
/// allowed; `Option<T>` is allowed.
pub struct RequestField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct ConfigField {
    pub name: syn::Ident,
    pub key: String,
    pub env_hint: String,
    pub ty_name: String,
}

pub struct ConfigSectionField {
    pub name: syn::Ident,
    pub ty: syn::Type,
    pub prefix: String,
}

pub struct ConsumerMethod {
    pub bus_field: String,
    pub topic: Option<String>,
    pub deserializer: Option<String>,
    pub filter: Option<String>,
    pub retry: Option<u32>,
    pub dlq: Option<String>,
    pub event_type: syn::Type,
    /// Whether the method is a plain fan-out subscriber (`-> ()` /
    /// `-> Result<(), E>`) or a request-reply responder (non-`()` return).
    pub kind: ConsumerKind,
    /// Method-level `#[intercept(...)]` sites. Controller-level intercepts are
    /// prepended (outermost) at codegen time, mirroring the scheduled path.
    pub intercept_fns: Vec<syn::Expr>,
    pub fn_item: syn::ImplItemFn,
}

/// How a `#[consumer]` method is wired against the [`EventBus`].
///
/// Determined from the method's return type: a `()` (or `Result<(), E>`)
/// return is a fan-out subscriber; any other return type is a Quarkus
/// `@ConsumeEvent`-style responder whose return value IS the reply.
#[derive(Clone)]
pub enum ConsumerKind {
    /// Fan-out subscriber — registered via `EventBus::subscribe`.
    Subscriber,
    /// Request-reply responder — registered via `EventBus::respond`. The
    /// method's return value is the reply payload.
    Responder {
        /// The reply payload type (`Resp`). For a fallible responder this is
        /// the `Ok` type unwrapped from `Result<Resp, E>`.
        resp_type: syn::Type,
        /// `true` when the method returns `Result<Resp, E>` (the `Err` is
        /// mapped to the responder error string → `EventBusError::Remote`).
        fallible: bool,
    },
}

#[derive(Clone)]
pub struct ScheduledConfig {
    /// Interval in milliseconds (parsed from integer seconds or duration string).
    pub every_ms: Option<u64>,
    pub cron: Option<String>,
    /// Initial delay in milliseconds (parsed from integer seconds or duration string).
    pub initial_delay_ms: Option<u64>,
    pub name: Option<String>,
    /// Self-overlap policy from `#[scheduled(overlap = "...")]` (default `Skip`).
    pub overlap: OverlapMode,
}

/// Parsed `overlap = "..."` value for `#[scheduled]` — mirrors
/// `r2e_scheduler::OverlapPolicy`.
#[derive(Default, Clone, Copy)]
pub enum OverlapMode {
    /// Never run a job concurrently with itself (default).
    #[default]
    Skip,
    /// Let a job overlap with itself.
    Concurrent,
}

pub struct ScheduledMethod {
    pub config: ScheduledConfig,
    pub intercept_fns: Vec<syn::Expr>,
    pub fn_item: syn::ImplItemFn,
}

/// Method annotated with `#[async_exec]` — submitted to a `PoolExecutor`.
pub struct AsyncExecMethod {
    /// Name of the controller field holding the executor. Default: `executor`.
    pub executor_field: syn::Ident,
    /// Original `async fn` body, kept as-is and rendered as
    /// `__r2e_async_<name>_inner` in the impl block.
    pub fn_item: syn::ImplItemFn,
}

/// Shared decorator data extracted from method attributes.
/// Used by RouteMethod, SseMethod, WsMethod, and GrpcMethod.
#[derive(Default)]
pub struct MethodDecorators {
    pub roles: Vec<String>,
    pub all_roles: Vec<String>,
    pub intercept_fns: Vec<syn::Expr>,
    pub guard_fns: Vec<syn::Expr>,
    pub pre_auth_guard_fns: Vec<syn::Expr>,
    pub middleware_fns: Vec<syn::Path>,
    pub layer_exprs: Vec<syn::Expr>,
    pub status_override: Option<u16>,
    pub returns_type: Option<syn::Type>,
    pub deprecated: bool,
    /// `#[anonymous]` — opt this route out of the controller's struct-level
    /// identity: no extraction runs and the method is emitted on the core
    /// (reading the identity field is a compile error).
    pub anonymous: bool,
}

pub struct IdentityParam {
    pub index: usize,
    /// The inner identity type (e.g. `AuthenticatedUser`), unwrapped from `Option<T>` if optional.
    pub ty: syn::Type,
    /// Whether the parameter was declared as `Option<T>`.
    pub is_optional: bool,
}

/// Parameter marked with `#[managed]` for automatic lifecycle management.
pub struct ManagedParam {
    pub index: usize,
    pub ty: syn::Type,
}

pub struct RouteMethod {
    pub method: HttpMethod,
    pub path: String,
    pub decorators: MethodDecorators,
    pub identity_param: Option<IdentityParam>,
    pub managed_params: Vec<ManagedParam>,
    pub fn_item: syn::ImplItemFn,
    /// `#[fallback]` route: registered via `Router::fallback` instead of
    /// `.route(path, ...)`. `method`/`path` are placeholders (`Any` / `""`).
    pub is_fallback: bool,
}

/// Keep-alive configuration for SSE endpoints.
pub enum SseKeepAlive {
    /// Use the default keep-alive (Axum default).
    Default,
    /// Custom interval in seconds.
    Interval(u64),
    /// Disable keep-alive.
    Disabled,
}

pub struct SseMethod {
    pub path: String,
    pub keep_alive: SseKeepAlive,
    pub decorators: MethodDecorators,
    pub identity_param: Option<IdentityParam>,
    pub fn_item: syn::ImplItemFn,
}

pub struct WsMethod {
    pub path: String,
    pub decorators: MethodDecorators,
    pub identity_param: Option<IdentityParam>,
    /// Index of the WsStream/WebSocket parameter among typed params, or None if returning WsHandler.
    pub ws_param: Option<WsParam>,
    pub fn_item: syn::ImplItemFn,
}

pub struct WsParam {
    pub index: usize,
    #[allow(dead_code)]
    pub ty: syn::Type,
    /// True if the type is WsStream (vs raw WebSocket).
    pub is_ws_stream: bool,
}
