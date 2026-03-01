use crate::route::HttpMethod;

pub struct InjectedField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct IdentityField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct ConfigField {
    pub name: syn::Ident,
    pub ty: syn::Type,
    pub key: String,
}

pub struct ConfigSectionField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct ConsumerMethod {
    pub bus_field: String,
    pub event_type: syn::Type,
    pub fn_item: syn::ImplItemFn,
}

pub struct ScheduledConfig {
    pub every: Option<u64>,
    pub cron: Option<String>,
    pub initial_delay: Option<u64>,
    pub name: Option<String>,
}

pub struct ScheduledMethod {
    pub config: ScheduledConfig,
    pub intercept_fns: Vec<syn::Expr>,
    pub fn_item: syn::ImplItemFn,
}

/// Shared decorator data extracted from method attributes.
/// Used by RouteMethod, SseMethod, WsMethod, and GrpcMethod.
#[derive(Default)]
pub struct MethodDecorators {
    pub roles: Vec<String>,
    pub transactional: Option<TransactionalConfig>,
    pub intercept_fns: Vec<syn::Expr>,
    pub guard_fns: Vec<syn::Expr>,
    pub pre_auth_guard_fns: Vec<syn::Expr>,
    pub middleware_fns: Vec<syn::Path>,
    pub layer_exprs: Vec<syn::Expr>,
    pub status_override: Option<u16>,
    pub returns_type: Option<syn::Type>,
    pub deprecated: bool,
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
}

pub struct TransactionalConfig {
    pub pool_field: String,
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
