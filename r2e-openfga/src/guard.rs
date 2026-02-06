//! OpenFGA guard for fine-grained authorization checks.

use crate::error::OpenFgaError;
use crate::registry::OpenFgaRegistry;
use r2e_core::guards::{Guard, GuardContext, Identity};
use r2e_core::http::extract::FromRef;
use r2e_core::http::response::IntoResponse;

/// How to resolve the object ID for authorization checks.
#[derive(Debug, Clone)]
pub enum ObjectResolver {
    /// Extract from a path parameter (e.g., `{doc_id}` in route).
    PathParam(&'static str),
    /// Extract from a query string parameter.
    QueryParam(&'static str),
    /// Extract from a header.
    Header(&'static str),
    /// Use a fixed object ID.
    Fixed(&'static str),
}

/// Builder for FGA check guards.
///
/// # Examples
///
/// ```ignore
/// use r2e_openfga::FgaCheck;
///
/// // Check using query parameter
/// #[guard(FgaCheck::relation("viewer").on("document").from_query("doc_id"))]
///
/// // Check using header
/// #[guard(FgaCheck::relation("editor").on("document").from_header("X-Document-Id"))]
///
/// // Check using fixed object
/// #[guard(FgaCheck::relation("member").on("organization").fixed("org:acme"))]
/// ```
pub struct FgaCheck;

impl FgaCheck {
    /// Start building a check for the given relation.
    pub fn relation(relation: &'static str) -> FgaCheckBuilder {
        FgaCheckBuilder { relation }
    }
}

/// Builder step: relation has been set, object type needed.
pub struct FgaCheckBuilder {
    relation: &'static str,
}

impl FgaCheckBuilder {
    /// Set the object type. You must then specify how to resolve the object ID.
    pub fn on(self, object_type: &'static str) -> FgaObjectBuilder {
        FgaObjectBuilder {
            relation: self.relation,
            object_type,
        }
    }
}

/// Builder step: object type has been set, resolution method needed.
pub struct FgaObjectBuilder {
    relation: &'static str,
    object_type: &'static str,
}

impl FgaObjectBuilder {
    /// Extract object ID from a path parameter.
    ///
    /// # Example
    /// ```ignore
    /// // GET /api/documents/{doc_id}
    /// #[guard(FgaCheck::relation("viewer").on("document").from_path("doc_id"))]
    /// ```
    pub fn from_path(self, param: &'static str) -> FgaGuard {
        FgaGuard {
            relation: self.relation,
            object_type: self.object_type,
            resolver: ObjectResolver::PathParam(param),
        }
    }

    /// Extract object ID from a query string parameter.
    ///
    /// # Example
    /// ```ignore
    /// // GET /api/documents?doc_id=123
    /// #[guard(FgaCheck::relation("viewer").on("document").from_query("doc_id"))]
    /// ```
    pub fn from_query(self, param: &'static str) -> FgaGuard {
        FgaGuard {
            relation: self.relation,
            object_type: self.object_type,
            resolver: ObjectResolver::QueryParam(param),
        }
    }

    /// Extract object ID from a request header.
    ///
    /// # Example
    /// ```ignore
    /// // Header: X-Document-Id: 123
    /// #[guard(FgaCheck::relation("viewer").on("document").from_header("X-Document-Id"))]
    /// ```
    pub fn from_header(self, header: &'static str) -> FgaGuard {
        FgaGuard {
            relation: self.relation,
            object_type: self.object_type,
            resolver: ObjectResolver::Header(header),
        }
    }

    /// Use a fixed object ID.
    ///
    /// # Example
    /// ```ignore
    /// #[guard(FgaCheck::relation("admin").on("system").fixed("system:global"))]
    /// ```
    pub fn fixed(self, object: &'static str) -> FgaGuard {
        FgaGuard {
            relation: self.relation,
            object_type: self.object_type,
            resolver: ObjectResolver::Fixed(object),
        }
    }
}

/// OpenFGA authorization guard.
///
/// Checks if the current user has the specified relation to an object.
/// The object ID is resolved from the request using the configured resolver.
pub struct FgaGuard {
    pub relation: &'static str,
    pub object_type: &'static str,
    pub resolver: ObjectResolver,
}

impl FgaGuard {
    /// Resolve the object ID from the request context.
    fn resolve_object<I: Identity>(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> Result<String, OpenFgaError> {
        let id = match &self.resolver {
            ObjectResolver::PathParam(param) => ctx
                .path_param(param)
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    OpenFgaError::ObjectResolutionFailed(format!(
                        "path param '{}' not found",
                        param
                    ))
                })?,
            ObjectResolver::QueryParam(param) => ctx
                .query_string()
                .and_then(|qs| {
                    url::form_urlencoded::parse(qs.as_bytes())
                        .find(|(k, _)| k == *param)
                        .map(|(_, v)| v.into_owned())
                })
                .ok_or_else(|| {
                    OpenFgaError::ObjectResolutionFailed(format!(
                        "query param '{}' not found",
                        param
                    ))
                })?,
            ObjectResolver::Header(header) => ctx
                .headers
                .get(*header)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    OpenFgaError::ObjectResolutionFailed(format!("header '{}' not found", header))
                })?,
            ObjectResolver::Fixed(object) => object.to_string(),
        };

        // Format as "type:id"
        if id.contains(':') {
            // Already formatted
            Ok(id)
        } else {
            Ok(format!("{}:{}", self.object_type, id))
        }
    }
}

impl<S: Send + Sync, I: Identity> Guard<S, I> for FgaGuard
where
    OpenFgaRegistry: FromRef<S>,
{
    fn check(
        &self,
        state: &S,
        ctx: &GuardContext<'_, I>,
    ) -> impl std::future::Future<Output = Result<(), r2e_core::http::Response>> + Send {
        let registry = <OpenFgaRegistry as FromRef<S>>::from_ref(state);
        let relation = self.relation;
        let object_result = self.resolve_object(ctx);

        // Get user ID from identity
        let user = ctx.identity.map(|i| format!("user:{}", i.sub()));

        async move {
            let user = user.ok_or_else(|| {
                (
                    r2e_core::http::StatusCode::UNAUTHORIZED,
                    r2e_core::http::Json(serde_json::json!({
                        "error": "Authentication required for authorization check"
                    })),
                )
                    .into_response()
            })?;

            let object = object_result.map_err(|e| {
                tracing::warn!(error = %e, "failed to resolve object for FGA check");
                (
                    r2e_core::http::StatusCode::BAD_REQUEST,
                    r2e_core::http::Json(serde_json::json!({
                        "error": format!("Failed to resolve object: {}", e)
                    })),
                )
                    .into_response()
            })?;

            tracing::debug!(
                user = %user,
                relation = %relation,
                object = %object,
                "checking authorization"
            );

            match registry.check(&user, relation, &object).await {
                Ok(true) => Ok(()),
                Ok(false) => {
                    tracing::debug!(
                        user = %user,
                        relation = %relation,
                        object = %object,
                        "authorization denied"
                    );
                    Err((
                        r2e_core::http::StatusCode::FORBIDDEN,
                        r2e_core::http::Json(serde_json::json!({
                            "error": "Access denied"
                        })),
                    )
                        .into_response())
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        user = %user,
                        relation = %relation,
                        object = %object,
                        "authorization check failed"
                    );
                    Err((
                        r2e_core::http::StatusCode::INTERNAL_SERVER_ERROR,
                        r2e_core::http::Json(serde_json::json!({
                            "error": "Authorization check failed"
                        })),
                    )
                        .into_response())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2e_core::guards::PathParams;
    use r2e_core::http::{HeaderMap, Uri};

    struct TestIdentity {
        sub: String,
    }

    impl Identity for TestIdentity {
        fn sub(&self) -> &str {
            &self.sub
        }
        fn roles(&self) -> &[String] {
            &[]
        }
    }

    #[test]
    fn test_fga_check_builder() {
        let guard = FgaCheck::relation("viewer").on("document").from_query("id");
        assert_eq!(guard.relation, "viewer");
        assert_eq!(guard.object_type, "document");
        assert!(matches!(guard.resolver, ObjectResolver::QueryParam("id")));
    }

    #[test]
    fn test_fga_check_builder_from_path() {
        let guard = FgaCheck::relation("viewer").on("document").from_path("doc_id");
        assert_eq!(guard.relation, "viewer");
        assert_eq!(guard.object_type, "document");
        assert!(matches!(guard.resolver, ObjectResolver::PathParam("doc_id")));
    }

    #[test]
    fn test_guard_with_fixed() {
        let guard = FgaCheck::relation("member")
            .on("organization")
            .fixed("org:acme");

        assert!(matches!(guard.resolver, ObjectResolver::Fixed("org:acme")));
    }

    #[test]
    fn test_resolve_object_from_path() {
        let guard = FgaCheck::relation("viewer")
            .on("document")
            .from_path("doc_id");

        let uri: Uri = "/api/documents/123".parse().unwrap();
        let headers = HeaderMap::new();
        let pairs = [("doc_id", "123")];
        let path_params = PathParams::from_pairs(&pairs);
        let identity = TestIdentity {
            sub: "alice".to_string(),
        };

        let ctx = GuardContext {
            method_name: "get",
            controller_name: "DocumentController",
            headers: &headers,
            uri: &uri,
            path_params,
            identity: Some(&identity),
        };

        let object = guard.resolve_object(&ctx).unwrap();
        assert_eq!(object, "document:123");
    }

    #[test]
    fn test_resolve_object_from_path_missing() {
        let guard = FgaCheck::relation("viewer")
            .on("document")
            .from_path("doc_id");

        let uri: Uri = "/api/documents/123".parse().unwrap();
        let headers = HeaderMap::new();
        let identity = TestIdentity {
            sub: "alice".to_string(),
        };

        let ctx = GuardContext {
            method_name: "get",
            controller_name: "DocumentController",
            headers: &headers,
            uri: &uri,
            path_params: PathParams::EMPTY,
            identity: Some(&identity),
        };

        let result = guard.resolve_object(&ctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_object_from_query() {
        let guard = FgaCheck::relation("viewer")
            .on("document")
            .from_query("doc_id");

        let uri: Uri = "/api/documents?doc_id=123&other=foo".parse().unwrap();
        let headers = HeaderMap::new();
        let identity = TestIdentity {
            sub: "alice".to_string(),
        };

        let ctx = GuardContext {
            method_name: "get",
            controller_name: "DocumentController",
            headers: &headers,
            uri: &uri,
            path_params: PathParams::EMPTY,
            identity: Some(&identity),
        };

        let object = guard.resolve_object(&ctx).unwrap();
        assert_eq!(object, "document:123");
    }

    #[test]
    fn test_resolve_object_from_header() {
        let guard = FgaCheck::relation("viewer")
            .on("document")
            .from_header("X-Document-Id");

        let uri: Uri = "/api/documents".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("X-Document-Id", "doc-999".parse().unwrap());
        let identity = TestIdentity {
            sub: "alice".to_string(),
        };

        let ctx = GuardContext {
            method_name: "get",
            controller_name: "DocumentController",
            headers: &headers,
            uri: &uri,
            path_params: PathParams::EMPTY,
            identity: Some(&identity),
        };

        let object = guard.resolve_object(&ctx).unwrap();
        assert_eq!(object, "document:doc-999");
    }

    #[test]
    fn test_resolve_object_fixed() {
        let guard = FgaCheck::relation("admin")
            .on("system")
            .fixed("system:global");

        let uri: Uri = "/api/admin".parse().unwrap();
        let headers = HeaderMap::new();
        let identity = TestIdentity {
            sub: "alice".to_string(),
        };

        let ctx = GuardContext {
            method_name: "get",
            controller_name: "AdminController",
            headers: &headers,
            uri: &uri,
            path_params: PathParams::EMPTY,
            identity: Some(&identity),
        };

        let object = guard.resolve_object(&ctx).unwrap();
        assert_eq!(object, "system:global");
    }

    #[test]
    fn test_resolve_object_query_missing() {
        let guard = FgaCheck::relation("viewer")
            .on("document")
            .from_query("doc_id");

        let uri: Uri = "/api/documents?other=foo".parse().unwrap();
        let headers = HeaderMap::new();
        let identity = TestIdentity {
            sub: "alice".to_string(),
        };

        let ctx = GuardContext {
            method_name: "get",
            controller_name: "DocumentController",
            headers: &headers,
            uri: &uri,
            path_params: PathParams::EMPTY,
            identity: Some(&identity),
        };

        let result = guard.resolve_object(&ctx);
        assert!(result.is_err());
    }
}
