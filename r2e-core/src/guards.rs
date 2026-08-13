use std::net::{IpAddr, SocketAddr};

use crate::http::response::Response;
use crate::http::{HeaderMap, Uri};

/// Typed descriptor for a route path parameter.
///
/// `#[routes]` generates values of this type in a local `path` namespace so
/// guard constructors can refer to path params without raw string literals:
///
/// ```ignore
/// #[guard(ProjectGuard::viewer(path::id))]
/// async fn show(&self, Path(id): Path<ProjectId>) { ... }
/// ```
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct PathParam<T = ()> {
    pub name: &'static str,
    _marker: std::marker::PhantomData<fn() -> T>,
}

impl<T> Clone for PathParam<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for PathParam<T> {}

impl<T> PathParam<T> {
    /// Create a typed descriptor for a named route path parameter.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _marker: std::marker::PhantomData,
        }
    }

    /// Return the route path parameter name.
    pub const fn name(self) -> &'static str {
        self.name
    }
}

impl<T> AsRef<str> for PathParam<T> {
    fn as_ref(&self) -> &str {
        self.name
    }
}

/// Trait representing an authenticated identity (user, service account, etc.).
///
/// Implement this trait on your identity type (e.g. `AuthenticatedUser`) to
/// decouple guards from a concrete identity struct.
///
/// For role-based access control, see `RoleBasedIdentity` in `r2e-security`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `Identity`",
    label = "this type cannot be used as an identity",
    note = "implement `Identity` for your type, or use `AuthenticatedUser` from `r2e-security` which implements it"
)]
pub trait Identity: Send + Sync {
    /// Unique subject identifier (e.g. JWT "sub" claim).
    fn sub(&self) -> &str;

    /// Email associated with this identity, if available.
    fn email(&self) -> Option<&str> {
        None
    }

    /// Raw JWT claims, if available.
    fn claims(&self) -> Option<&serde_json::Value> {
        None
    }
}

/// Sentinel type representing the absence of an identity.
///
/// Used as the default `IdentityType` in controllers that have no
/// `#[inject(identity)]` field.
pub struct NoIdentity;

impl Identity for NoIdentity {
    fn sub(&self) -> &str {
        ""
    }
}

/// Path parameters extracted from the matched route pattern.
///
/// In production, this borrows Axum's `RawPathParams` with zero copy.
/// For testing, construct via [`PathParams::from_pairs`].
pub struct PathParams<'a>(PathParamsInner<'a>);

enum PathParamsInner<'a> {
    Raw(&'a crate::http::extract::RawPathParams),
    Pairs(&'a [(&'a str, &'a str)]),
}

impl<'a> PathParams<'a> {
    /// Create from Axum's `RawPathParams` (zero copy, used by generated code).
    pub fn from_raw(raw: &'a crate::http::extract::RawPathParams) -> Self {
        Self(PathParamsInner::Raw(raw))
    }

    /// Create from a slice of `(key, value)` pairs (for testing).
    pub fn from_pairs(pairs: &'a [(&'a str, &'a str)]) -> Self {
        Self(PathParamsInner::Pairs(pairs))
    }

    /// Empty path params (convenience for contexts without route matching).
    pub const EMPTY: PathParams<'static> = PathParams(PathParamsInner::Pairs(&[]));

    /// Get a path parameter by name.
    ///
    /// Linear scan — optimal for the typical 1-3 path params.
    ///
    /// # Example
    /// ```ignore
    /// // For route `/orgs/{org_id}/documents/{doc_id}`
    /// // and request path `/orgs/acme/documents/123`
    /// ctx.path_params.get("org_id")  // => Some("acme")
    /// ctx.path_params.get("doc_id")  // => Some("123")
    /// ```
    pub fn get(&self, name: &str) -> Option<&str> {
        match &self.0 {
            PathParamsInner::Raw(raw) => raw.iter().find(|(k, _)| *k == name).map(|(_, v)| v),
            PathParamsInner::Pairs(pairs) => {
                pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
            }
        }
    }

    /// Parse a path parameter into a strongly typed value.
    ///
    /// Missing parameters indicate a guard/route mismatch and return a 500
    /// [`GuardError`]. Values that fail to parse return a 400 [`GuardError`].
    ///
    /// # Example
    /// ```ignore
    /// let id: u64 = ctx.path_params.parse("id")?;
    /// ```
    pub fn parse<T>(&self, name: &str) -> Result<T, GuardError>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        let value = self
            .get(name)
            .ok_or_else(|| GuardError::missing_path_param(name))?;
        value
            .parse()
            .map_err(|err| GuardError::invalid_path_param(name, value, err))
    }
}

/// Where a resolved client IP came from.
///
/// Returned by [`GuardContext::client_ip`] / [`PreAuthGuardContext::client_ip`].
/// Both variants carry a parsed [`IpAddr`], so `Display` renders the **canonical**
/// address (no port, IPv6 aliases collapsed) — which is what IP-keyed guards
/// (rate limiting, allowlists) should use as a bucket key.
///
/// **Trust model:** `Forwarded` is *client-controlled input* unless a reverse
/// proxy **overwrites** `X-Forwarded-For` (a proxy that *appends* leaves the
/// leftmost entry forgeable). `Peer` is the transport source address — never
/// forgeable, but it is the proxy's address when the app sits behind one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientIp {
    /// Leftmost entry of the `X-Forwarded-For` header, parsed and canonicalized.
    Forwarded(IpAddr),
    /// Transport peer address (from `ConnectInfo`), port dropped.
    Peer(IpAddr),
}

impl ClientIp {
    /// The address, regardless of where it came from.
    pub fn ip(self) -> IpAddr {
        match self {
            ClientIp::Forwarded(addr) | ClientIp::Peer(addr) => addr,
        }
    }
}

impl std::fmt::Display for ClientIp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.ip())
    }
}

/// Parse one `X-Forwarded-For` entry into a canonical [`IpAddr`].
///
/// The header is attacker-controlled text: anything that is not a valid address
/// must be treated as **absent** (so callers fall through to the transport peer
/// address) rather than used verbatim as an identity — otherwise a client can
/// mint an unbounded number of rate-limit buckets, and `::1` / `0:0:0:0:0:0:0:1`
/// / `[::1]:8080` would each get their own.
///
/// Accepted forms (whitespace trimmed):
///
/// | Input | Result |
/// |---|---|
/// | `1.2.3.4` | `1.2.3.4` |
/// | `1.2.3.4:5678` | `1.2.3.4` (port dropped) |
/// | `2001:db8::1` | `2001:db8::1` |
/// | `0:0:0:0:0:0:0:1` | `::1` (canonicalized) |
/// | `[::1]` / `[::1]:8080` | `::1` |
/// | `unknown`, `_hidden`, `bob`, `` | `None` |
pub fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    let raw = value.trim();
    if raw.is_empty() {
        return None;
    }
    // Bare address: `1.2.3.4`, `2001:db8::1`, `0:0:0:0:0:0:0:1`.
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Some(ip);
    }
    // With a port: `1.2.3.4:5678`, `[::1]:8080`.
    if let Ok(addr) = raw.parse::<SocketAddr>() {
        return Some(addr.ip());
    }
    // Bracketed IPv6 without a port: `[::1]`.
    if let Some(inner) = raw.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        if let Ok(ip) = inner.parse::<IpAddr>() {
            return Some(ip);
        }
    }
    None
}

/// Leftmost `X-Forwarded-For` entry, if the header is present and non-empty.
///
/// Raw, **unvalidated** text — see [`forwarded_ip`] for the parsed form.
fn forwarded_for(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Leftmost `X-Forwarded-For` entry parsed into a canonical [`IpAddr`].
///
/// Only the leftmost entry is considered (the client, when the proxy overwrites
/// the header). A malformed leftmost entry yields `None` — it is never repaired
/// by scanning further right, since everything to its right was contributed by
/// the same untrusted hop.
fn forwarded_ip(headers: &HeaderMap) -> Option<IpAddr> {
    forwarded_for(headers).and_then(parse_forwarded_ip)
}

/// Context available to guards before the handler body runs.
///
/// Generic over the identity type `I` so that guards can access the full
/// identity object (not just sub/roles strings).
pub struct GuardContext<'a, I: Identity> {
    pub method_name: &'static str,
    pub controller_name: &'static str,
    pub headers: &'a HeaderMap,
    pub uri: &'a Uri,
    /// Transport peer address, when the server was started with connection
    /// info (`serve_auto` / the sharded server do). `None` under `TestApp`'s
    /// in-process dispatch and for any transport that does not record it.
    pub peer_addr: Option<SocketAddr>,
    pub path_params: PathParams<'a>,
    pub identity: Option<&'a I>,
}

impl<'a, I: Identity> GuardContext<'a, I> {
    /// Convenience accessor for the identity subject.
    pub fn identity_sub(&self) -> Option<&str> {
        self.identity.map(|i| i.sub())
    }

    /// The request path.
    pub fn path(&self) -> &str {
        self.uri.path()
    }

    /// The request query string, if any.
    pub fn query_string(&self) -> Option<&str> {
        self.uri.query()
    }

    /// Get a path parameter by name.
    ///
    /// # Example
    /// ```ignore
    /// // For route `/orgs/{org_id}/documents/{doc_id}`
    /// // and request path `/orgs/acme/documents/123`
    /// ctx.path_param("org_id")  // => Some("acme")
    /// ctx.path_param("doc_id")  // => Some("123")
    /// ```
    pub fn path_param(&self, name: &str) -> Option<&str> {
        self.path_params.get(name)
    }

    /// Parse a path parameter into a strongly typed value.
    ///
    /// Use this in resource guards to avoid repeating string lookup and parse
    /// boilerplate.
    ///
    /// # Example
    /// ```ignore
    /// let project_id: ProjectId = ctx.parse_path_param("pid")?;
    /// ```
    pub fn parse_path_param<T>(&self, name: &str) -> Result<T, GuardError>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        self.path_params.parse(name)
    }

    /// Leftmost `X-Forwarded-For` entry, **raw and unvalidated**.
    ///
    /// Only trustworthy when a reverse proxy **overwrites** the header, and even
    /// then it is arbitrary client text. Use [`forwarded_ip`](Self::forwarded_ip)
    /// (or [`client_ip`](Self::client_ip)) for anything that keys on the value.
    pub fn forwarded_for(&self) -> Option<&str> {
        forwarded_for(self.headers)
    }

    /// Leftmost `X-Forwarded-For` entry parsed into a canonical [`IpAddr`].
    ///
    /// `None` when the header is absent, empty, or does not parse as an address
    /// (see [`parse_forwarded_ip`]).
    pub fn forwarded_ip(&self) -> Option<IpAddr> {
        forwarded_ip(self.headers)
    }

    /// Transport peer IP (port dropped), if the server recorded it.
    pub fn peer_ip(&self) -> Option<IpAddr> {
        self.peer_addr.map(|addr| addr.ip())
    }

    /// The client IP: leftmost **parseable** `X-Forwarded-For` entry when
    /// present, else the transport peer IP. `None` when neither is available.
    ///
    /// A malformed `X-Forwarded-For` never suppresses the peer fallback.
    /// See [`ClientIp`] for the trust model.
    pub fn client_ip(&self) -> Option<ClientIp> {
        self.forwarded_ip()
            .map(ClientIp::Forwarded)
            .or_else(|| self.peer_ip().map(ClientIp::Peer))
    }

    /// Convenience accessor for the identity email.
    pub fn identity_email(&self) -> Option<&str> {
        self.identity.and_then(|i| i.email())
    }

    /// Convenience accessor for the identity raw claims.
    pub fn identity_claims(&self) -> Option<&serde_json::Value> {
        self.identity.and_then(|i| i.claims())
    }
}

/// Handler-level guard. Runs before the handler body.
/// Returns `Ok(())` to proceed, `Err(Response)` to short-circuit.
///
/// Guards are the handler-level counterpart of `Interceptor<R>` (which is method-level).
/// Built-in guards: `RolesGuard` (in `r2e-security`), `RateLimitGuard` (in `r2e-rate-limit`).
/// Users can implement custom guards and apply them with `#[guard(expr)]`.
///
/// Guards are built **once, at controller registration**, from the resolved
/// bean graph (see [`DecoratorSpec`](crate::decorator::DecoratorSpec)) — a
/// guard that reads beans holds them as fields; there is no state access at
/// request time. Generic over the identity type `I`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `Guard<{I}>`",
    label = "this type cannot be used as a guard",
    note = "implement `Guard<I>` for your type; if it reads beans, hold them as fields and implement `DecoratorSpec` on its config type, otherwise add `impl SelfBuilt for {Self} {{}}`"
)]
pub trait Guard<I: Identity>: Send + Sync {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send;
}

/// Context available to pre-authentication guards.
///
/// Unlike [`GuardContext`], this does not carry identity information — it runs
/// before JWT extraction/validation.
pub struct PreAuthGuardContext<'a> {
    pub method_name: &'static str,
    pub controller_name: &'static str,
    pub headers: &'a HeaderMap,
    pub uri: &'a Uri,
    /// Transport peer address, when the server was started with connection
    /// info (`serve_auto` / the sharded server do). `None` under `TestApp`'s
    /// in-process dispatch and for any transport that does not record it.
    pub peer_addr: Option<SocketAddr>,
    pub path_params: PathParams<'a>,
}

impl<'a> PreAuthGuardContext<'a> {
    /// The request path.
    pub fn path(&self) -> &str {
        self.uri.path()
    }

    /// Leftmost `X-Forwarded-For` entry, **raw and unvalidated**.
    ///
    /// Only trustworthy when a reverse proxy **overwrites** the header, and even
    /// then it is arbitrary client text. Use [`forwarded_ip`](Self::forwarded_ip)
    /// (or [`client_ip`](Self::client_ip)) for anything that keys on the value.
    pub fn forwarded_for(&self) -> Option<&str> {
        forwarded_for(self.headers)
    }

    /// Leftmost `X-Forwarded-For` entry parsed into a canonical [`IpAddr`].
    ///
    /// `None` when the header is absent, empty, or does not parse as an address
    /// (see [`parse_forwarded_ip`]).
    pub fn forwarded_ip(&self) -> Option<IpAddr> {
        forwarded_ip(self.headers)
    }

    /// Transport peer IP (port dropped), if the server recorded it.
    pub fn peer_ip(&self) -> Option<IpAddr> {
        self.peer_addr.map(|addr| addr.ip())
    }

    /// The client IP: leftmost **parseable** `X-Forwarded-For` entry when
    /// present, else the transport peer IP. `None` when neither is available.
    ///
    /// A malformed `X-Forwarded-For` never suppresses the peer fallback.
    /// See [`ClientIp`] for the trust model.
    pub fn client_ip(&self) -> Option<ClientIp> {
        self.forwarded_ip()
            .map(ClientIp::Forwarded)
            .or_else(|| self.peer_ip().map(ClientIp::Peer))
    }

    /// The request query string, if any.
    pub fn query_string(&self) -> Option<&str> {
        self.uri.query()
    }

    /// Get a path parameter by name.
    ///
    /// # Example
    /// ```ignore
    /// // For route `/orgs/{org_id}/documents/{doc_id}`
    /// // and request path `/orgs/acme/documents/123`
    /// ctx.path_param("org_id")  // => Some("acme")
    /// ctx.path_param("doc_id")  // => Some("123")
    /// ```
    pub fn path_param(&self, name: &str) -> Option<&str> {
        self.path_params.get(name)
    }

    /// Parse a path parameter into a strongly typed value.
    ///
    /// Missing parameters indicate a guard/route mismatch and return a 500
    /// [`GuardError`]. Values that fail to parse return a 400 [`GuardError`].
    pub fn parse_path_param<T>(&self, name: &str) -> Result<T, GuardError>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        self.path_params.parse(name)
    }
}

// ── GuardError helper ─────────────────────────────────────────────────

/// A convenient error type for guard implementations.
///
/// Instead of constructing `Response` manually, guards can return
/// `GuardError` and convert it with `.into()`.
///
/// # Example
/// ```ignore
/// use r2e_core::guards::GuardError;
///
/// async fn check(&self, ctx: &GuardContext<'_, I>) -> Result<(), Response> {
///     if ctx.identity.is_none() {
///         return Err(GuardError::new(StatusCode::FORBIDDEN, "access denied").into());
///     }
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct GuardError {
    pub status: crate::http::StatusCode,
    pub message: String,
}

impl GuardError {
    /// Create a new guard error with the given status and message.
    pub fn new(status: crate::http::StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// 401 Unauthorized guard error.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(crate::http::StatusCode::UNAUTHORIZED, message)
    }

    /// 403 Forbidden guard error.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(crate::http::StatusCode::FORBIDDEN, message)
    }

    /// 400 Bad Request guard error.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(crate::http::StatusCode::BAD_REQUEST, message)
    }

    /// 500 Internal Server Error guard error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(crate::http::StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// Error for a guard that references a route path parameter that does not exist.
    pub fn missing_path_param(name: &str) -> Self {
        Self::internal(format!(
            "missing path parameter `{name}` while evaluating guard"
        ))
    }

    /// Error for a path parameter that cannot be parsed as the requested type.
    pub fn invalid_path_param(name: &str, value: &str, err: impl std::fmt::Display) -> Self {
        Self::bad_request(format!(
            "invalid path parameter `{name}` value `{value}`: {err}"
        ))
    }
}

impl From<GuardError> for Response {
    fn from(err: GuardError) -> Self {
        crate::error::error_response(err.status, err.message)
    }
}

/// Guard that runs **before** authentication (JWT extraction).
///
/// Use this for checks that don't need identity (e.g., global or IP-based rate limiting).
/// This avoids wasting effort on JWT validation when the request will be rejected anyway.
///
/// Like [`Guard`], pre-auth guards are built once at registration from the
/// resolved bean graph — bean deps are fields, not state lookups.
///
/// Apply via `#[pre_guard(MyPreGuard)]` or automatically for `#[rate_limited]` with
/// `key = "global"` or `key = "ip"`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `PreAuthGuard`",
    label = "this type cannot be used as a pre-auth guard",
    note = "implement `PreAuthGuard` for your type and apply it with `#[pre_guard(YourGuard)]`"
)]
pub trait PreAuthGuard: Send + Sync {
    fn check(
        &self,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send;
}
