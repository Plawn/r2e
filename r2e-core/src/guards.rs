use crate::http::response::{IntoResponse, Response};
use crate::http::{HeaderMap, Uri};

/// Trait representing an authenticated identity (user, service account, etc.).
///
/// Implement this trait on your identity type (e.g. `AuthenticatedUser`) to
/// decouple guards from a concrete identity struct.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `Identity`",
    label = "this type cannot be used as an identity",
    note = "implement `Identity` for your type, or use `AuthenticatedUser` from `r2e-security` which implements it"
)]
pub trait Identity: Send + Sync {
    /// Unique subject identifier (e.g. JWT "sub" claim).
    fn sub(&self) -> &str;

    /// Roles associated with this identity.
    fn roles(&self) -> &[String];

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
    fn roles(&self) -> &[String] {
        &[]
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
            PathParamsInner::Raw(raw) => {
                raw.iter().find(|(k, _)| *k == name).map(|(_, v)| v)
            }
            PathParamsInner::Pairs(pairs) => {
                pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
            }
        }
    }
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
    pub path_params: PathParams<'a>,
    pub identity: Option<&'a I>,
}

impl<'a, I: Identity> GuardContext<'a, I> {
    /// Convenience accessor for the identity subject.
    pub fn identity_sub(&self) -> Option<&str> {
        self.identity.map(|i| i.sub())
    }

    /// Convenience accessor for the identity roles.
    pub fn identity_roles(&self) -> Option<&[String]> {
        self.identity.map(|i| i.roles())
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
/// Built-in guards: `RolesGuard`, `RateLimitGuard` (in `r2e-rate-limit`).
/// Users can implement custom guards and apply them with `#[guard(expr)]`.
///
/// Generic over both the application state `S` and the identity type `I`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `Guard<{S}, {I}>`",
    label = "this type cannot be used as a guard",
    note = "implement `Guard<S, I>` for your type and apply it with `#[guard(YourGuard)]`"
)]
pub trait Guard<S, I: Identity>: Send + Sync {
    fn check(
        &self,
        state: &S,
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
    pub path_params: PathParams<'a>,
}

impl<'a> PreAuthGuardContext<'a> {
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
}

/// Guard that runs **before** authentication (JWT extraction).
///
/// Use this for checks that don't need identity (e.g., global or IP-based rate limiting).
/// This avoids wasting effort on JWT validation when the request will be rejected anyway.
///
/// Apply via `#[pre_guard(MyPreGuard)]` or automatically for `#[rate_limited]` with
/// `key = "global"` or `key = "ip"`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `PreAuthGuard<{S}>`",
    label = "this type cannot be used as a pre-auth guard",
    note = "implement `PreAuthGuard<S>` for your type and apply it with `#[pre_guard(YourGuard)]`"
)]
pub trait PreAuthGuard<S>: Send + Sync {
    fn check(
        &self,
        state: &S,
        ctx: &PreAuthGuardContext<'_>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send;
}

/// Guard that checks required roles. Returns 403 if missing.
pub struct RolesGuard {
    pub required_roles: &'static [&'static str],
}

impl<S: Send + Sync, I: Identity> Guard<S, I> for RolesGuard {
    fn check(
        &self,
        _state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), Response>> + Send {
        let result = (|| {
            let identity = ctx.identity.ok_or_else(|| {
                crate::AppError::Forbidden("No identity available for role check".into())
                    .into_response()
            })?;
            let roles = identity.roles();
            let has_role = self
                .required_roles
                .iter()
                .any(|req| roles.iter().any(|r| r.as_str() == *req));
            if has_role {
                Ok(())
            } else {
                Err(crate::AppError::Forbidden("Insufficient roles".into()).into_response())
            }
        })();
        std::future::ready(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestIdentity {
        sub: String,
        roles: Vec<String>,
        email: Option<String>,
        claims: Option<serde_json::Value>,
    }

    impl TestIdentity {
        fn new(sub: &str, roles: &[&str]) -> Self {
            Self {
                sub: sub.to_string(),
                roles: roles.iter().map(|r| r.to_string()).collect(),
                email: None,
                claims: None,
            }
        }

        fn with_email(mut self, email: &str) -> Self {
            self.email = Some(email.to_string());
            self
        }
    }

    impl Identity for TestIdentity {
        fn sub(&self) -> &str {
            &self.sub
        }
        fn roles(&self) -> &[String] {
            &self.roles
        }
        fn email(&self) -> Option<&str> {
            self.email.as_deref()
        }
        fn claims(&self) -> Option<&serde_json::Value> {
            self.claims.as_ref()
        }
    }

    fn make_uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    fn make_ctx<'a, I: Identity>(
        identity: Option<&'a I>,
        uri: &'a Uri,
        headers: &'a HeaderMap,
        path_params: PathParams<'a>,
    ) -> GuardContext<'a, I> {
        GuardContext {
            method_name: "test_method",
            controller_name: "TestController",
            headers,
            uri,
            path_params,
            identity,
        }
    }

    // PathParams tests
    #[test]
    fn path_params_get_existing() {
        let pairs = [("id", "123")];
        let params = PathParams::from_pairs(&pairs);
        assert_eq!(params.get("id"), Some("123"));
    }

    #[test]
    fn path_params_get_missing() {
        let pairs = [("id", "123")];
        let params = PathParams::from_pairs(&pairs);
        assert_eq!(params.get("other"), None);
    }

    #[test]
    fn path_params_empty() {
        assert_eq!(PathParams::EMPTY.get("anything"), None);
    }

    // NoIdentity tests
    #[test]
    fn no_identity_sub_is_empty() {
        assert_eq!(NoIdentity.sub(), "");
    }

    #[test]
    fn no_identity_roles_is_empty() {
        assert!(NoIdentity.roles().is_empty());
    }

    // GuardContext accessor tests
    #[test]
    fn guard_context_identity_sub() {
        let id = TestIdentity::new("user-1", &["admin"]);
        let uri = make_uri("/test");
        let headers = HeaderMap::new();
        let ctx = make_ctx(Some(&id), &uri, &headers, PathParams::EMPTY);
        assert_eq!(ctx.identity_sub(), Some("user-1"));
    }

    #[test]
    fn guard_context_identity_roles() {
        let id = TestIdentity::new("user-1", &["admin", "editor"]);
        let uri = make_uri("/test");
        let headers = HeaderMap::new();
        let ctx = make_ctx(Some(&id), &uri, &headers, PathParams::EMPTY);
        let roles = ctx.identity_roles().unwrap();
        assert_eq!(roles.len(), 2);
        assert_eq!(roles[0], "admin");
        assert_eq!(roles[1], "editor");
    }

    #[test]
    fn guard_context_identity_email() {
        let id = TestIdentity::new("user-1", &[]).with_email("a@b.com");
        let uri = make_uri("/test");
        let headers = HeaderMap::new();
        let ctx = make_ctx(Some(&id), &uri, &headers, PathParams::EMPTY);
        assert_eq!(ctx.identity_email(), Some("a@b.com"));
    }

    #[test]
    fn guard_context_identity_none() {
        let uri = make_uri("/test");
        let headers = HeaderMap::new();
        let ctx: GuardContext<'_, TestIdentity> = make_ctx(None, &uri, &headers, PathParams::EMPTY);
        assert_eq!(ctx.identity_sub(), None);
        assert_eq!(ctx.identity_roles(), None);
        assert_eq!(ctx.identity_email(), None);
    }

    #[test]
    fn guard_context_path() {
        let uri = make_uri("/users?q=1");
        let headers = HeaderMap::new();
        let ctx: GuardContext<'_, NoIdentity> = make_ctx(None, &uri, &headers, PathParams::EMPTY);
        assert_eq!(ctx.path(), "/users");
    }

    #[test]
    fn guard_context_query_string() {
        let uri = make_uri("/users?q=1");
        let headers = HeaderMap::new();
        let ctx: GuardContext<'_, NoIdentity> = make_ctx(None, &uri, &headers, PathParams::EMPTY);
        assert_eq!(ctx.query_string(), Some("q=1"));
    }

    #[test]
    fn guard_context_path_param() {
        let pairs = [("id", "42")];
        let uri = make_uri("/users/42");
        let headers = HeaderMap::new();
        let ctx: GuardContext<'_, NoIdentity> =
            make_ctx(None, &uri, &headers, PathParams::from_pairs(&pairs));
        assert_eq!(ctx.path_param("id"), Some("42"));
        assert_eq!(ctx.path_param("missing"), None);
    }

    // RolesGuard tests
    #[tokio::test]
    async fn roles_guard_passes() {
        let guard = RolesGuard {
            required_roles: &["admin"],
        };
        let id = TestIdentity::new("user-1", &["admin", "user"]);
        let uri = make_uri("/test");
        let headers = HeaderMap::new();
        let ctx = make_ctx(Some(&id), &uri, &headers, PathParams::EMPTY);
        let result = guard.check(&(), &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn roles_guard_rejects() {
        let guard = RolesGuard {
            required_roles: &["admin"],
        };
        let id = TestIdentity::new("user-1", &["user"]);
        let uri = make_uri("/test");
        let headers = HeaderMap::new();
        let ctx = make_ctx(Some(&id), &uri, &headers, PathParams::EMPTY);
        let result = guard.check(&(), &ctx).await;
        assert!(result.is_err());
        let resp = result.unwrap_err();
        assert_eq!(resp.status(), crate::http::StatusCode::FORBIDDEN);
    }
}
