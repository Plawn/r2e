use r2e_core::beans::BeanRegistry;
use r2e_core::guards::{Guard, GuardContext, Identity, PathParams};
use r2e_core::http::{HeaderMap, StatusCode, Uri};
use r2e_core::DecoratorSpec;
use r2e_openfga::guard::{FgaCheck, FgaGuard, ObjectResolver};
use r2e_openfga::{MockBackend, OpenFgaRegistry};

struct TestIdentity {
    sub: String,
}

impl Identity for TestIdentity {
    fn sub(&self) -> &str {
        &self.sub
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
    let guard = FgaCheck::relation("viewer")
        .on("document")
        .from_path("doc_id");
    assert_eq!(guard.relation, "viewer");
    assert_eq!(guard.object_type, "document");
    assert!(matches!(
        guard.resolver,
        ObjectResolver::PathParam("doc_id")
    ));
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

#[test]
fn test_resolve_object_rejects_colon_in_path() {
    let guard = FgaCheck::relation("viewer")
        .on("document")
        .from_path("doc_id");

    let uri: Uri = "/api/documents/secret:admin".parse().unwrap();
    let headers = HeaderMap::new();
    let pairs = [("doc_id", "secret:admin")];
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

    let result = guard.resolve_object(&ctx);
    assert!(result.is_err());
}

#[test]
fn test_resolve_object_rejects_colon_in_query() {
    let guard = FgaCheck::relation("viewer")
        .on("document")
        .from_query("doc_id");

    let uri: Uri = "/api/documents?doc_id=secret:admin".parse().unwrap();
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
fn test_resolve_object_rejects_colon_in_header() {
    let guard = FgaCheck::relation("viewer")
        .on("document")
        .from_header("X-Document-Id");

    let uri: Uri = "/api/documents".parse().unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("X-Document-Id", "secret:admin".parse().unwrap());
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
fn test_resolve_object_fixed_allows_colon() {
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

    // Fixed values are developer-controlled, colons are allowed
    let object = guard.resolve_object(&ctx).unwrap();
    assert_eq!(object, "system:global");
}

// ── Built guard: DecoratorSpec::build + Guard::check ────────────────────────

async fn build_guard(mock: MockBackend, config: FgaCheck) -> FgaGuard {
    let mut registry = BeanRegistry::new();
    registry.provide(OpenFgaRegistry::new(mock));
    let ctx = registry.resolve().await.expect("graph must resolve");
    <FgaCheck as DecoratorSpec>::build(config, &ctx)
}

#[tokio::test]
async fn built_guard_allows_when_tuple_present() {
    let mock = MockBackend::new();
    mock.add_tuple("user:alice", "viewer", "document:123");
    let guard = build_guard(
        mock,
        FgaCheck::relation("viewer")
            .on("document")
            .from_path("doc_id"),
    )
    .await;

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

    assert!(guard.check(&ctx).await.is_ok());
}

#[tokio::test]
async fn built_guard_forbids_when_tuple_absent() {
    let mock = MockBackend::new();
    let guard = build_guard(
        mock,
        FgaCheck::relation("viewer")
            .on("document")
            .from_path("doc_id"),
    )
    .await;

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

    let response = guard.check(&ctx).await.expect_err("should be denied");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn built_guard_unauthorized_without_identity() {
    let mock = MockBackend::new();
    let guard = build_guard(
        mock,
        FgaCheck::relation("viewer")
            .on("document")
            .from_path("doc_id"),
    )
    .await;

    let uri: Uri = "/api/documents/123".parse().unwrap();
    let headers = HeaderMap::new();
    let pairs = [("doc_id", "123")];
    let path_params = PathParams::from_pairs(&pairs);
    let ctx: GuardContext<'_, TestIdentity> = GuardContext {
        method_name: "get",
        controller_name: "DocumentController",
        headers: &headers,
        uri: &uri,
        path_params,
        identity: None,
    };

    let response = guard.check(&ctx).await.expect_err("should be unauthorized");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// A crafted subject must never be interpolated into `user:{sub}`:
/// `*` would collapse onto public-wildcard grants, `:` would cross types,
/// `#` would form a userset reference. Fail closed with 403.
#[tokio::test]
async fn built_guard_forbids_subject_with_reserved_characters() {
    for sub in ["*", "alice#owner", "team:eng"] {
        let mock = MockBackend::new();
        // A wildcard grant exists — a forged `sub = "*"` must NOT match it.
        mock.add_tuple("user:*", "viewer", "document:123");
        let guard = build_guard(
            mock,
            FgaCheck::relation("viewer")
                .on("document")
                .from_path("doc_id"),
        )
        .await;

        let uri: Uri = "/api/documents/123".parse().unwrap();
        let headers = HeaderMap::new();
        let pairs = [("doc_id", "123")];
        let path_params = PathParams::from_pairs(&pairs);
        let identity = TestIdentity {
            sub: sub.to_string(),
        };
        let ctx = GuardContext {
            method_name: "get",
            controller_name: "DocumentController",
            headers: &headers,
            uri: &uri,
            path_params,
            identity: Some(&identity),
        };

        let response = guard.check(&ctx).await.expect_err("must be rejected");
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "sub = {sub:?}");
    }
}
