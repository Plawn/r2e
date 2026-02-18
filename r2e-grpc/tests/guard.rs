use r2e_core::guards::Identity;
use r2e_grpc::guard::{GrpcGuard, GrpcGuardContext, GrpcRolesGuard, GrpcRoleBasedIdentity};
use tonic::metadata::MetadataMap;

struct TestIdentity {
    sub: String,
    roles: Vec<String>,
}

impl TestIdentity {
    fn new(sub: &str, roles: &[&str]) -> Self {
        Self {
            sub: sub.to_string(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
        }
    }
}

impl Identity for TestIdentity {
    fn sub(&self) -> &str {
        &self.sub
    }
}

impl GrpcRoleBasedIdentity for TestIdentity {
    fn roles(&self) -> &[String] {
        &self.roles
    }
}

fn make_ctx<'a, I: Identity>(
    identity: Option<&'a I>,
    metadata: &'a MetadataMap,
) -> GrpcGuardContext<'a, I> {
    GrpcGuardContext {
        service_name: "test.TestService",
        method_name: "test_method",
        metadata,
        identity,
    }
}

#[tokio::test]
async fn roles_guard_passes_with_correct_role() {
    let guard = GrpcRolesGuard {
        required_roles: &["admin"],
    };
    let id = TestIdentity::new("user-1", &["admin", "user"]);
    let metadata = MetadataMap::new();
    let ctx = make_ctx(Some(&id), &metadata);
    let result = guard.check(&(), &ctx).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn roles_guard_rejects_missing_role() {
    let guard = GrpcRolesGuard {
        required_roles: &["admin"],
    };
    let id = TestIdentity::new("user-1", &["user"]);
    let metadata = MetadataMap::new();
    let ctx = make_ctx(Some(&id), &metadata);
    let result = guard.check(&(), &ctx).await;
    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::PermissionDenied);
}

#[tokio::test]
async fn roles_guard_rejects_no_identity() {
    let guard = GrpcRolesGuard {
        required_roles: &["admin"],
    };
    let metadata = MetadataMap::new();
    let ctx: GrpcGuardContext<'_, TestIdentity> = make_ctx(None, &metadata);
    let result = guard.check(&(), &ctx).await;
    assert!(result.is_err());
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

#[tokio::test]
async fn roles_guard_passes_with_any_required_role() {
    let guard = GrpcRolesGuard {
        required_roles: &["admin", "moderator"],
    };
    let id = TestIdentity::new("user-1", &["moderator"]);
    let metadata = MetadataMap::new();
    let ctx = make_ctx(Some(&id), &metadata);
    let result = guard.check(&(), &ctx).await;
    assert!(result.is_ok());
}

#[test]
fn guard_context_accessors() {
    let id = TestIdentity::new("user-42", &["admin"]);
    let metadata = MetadataMap::new();
    let ctx = make_ctx(Some(&id), &metadata);

    assert_eq!(ctx.identity_sub(), Some("user-42"));
    // identity_email returns None since TestIdentity doesn't override it
    assert_eq!(ctx.identity_email(), None);
    // identity_claims returns None since TestIdentity doesn't override it
    assert!(ctx.identity_claims().is_none());
    assert_eq!(ctx.service_name, "test.TestService");
    assert_eq!(ctx.method_name, "test_method");
}

#[test]
fn guard_context_no_identity_accessors() {
    let metadata = MetadataMap::new();
    let ctx: GrpcGuardContext<'_, TestIdentity> = make_ctx(None, &metadata);

    assert_eq!(ctx.identity_sub(), None);
    assert_eq!(ctx.identity_email(), None);
    assert!(ctx.identity_claims().is_none());
}
