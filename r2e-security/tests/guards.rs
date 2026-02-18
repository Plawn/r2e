use r2e_core::guards::{Guard, GuardContext, Identity, PathParams};
use r2e_core::http::{HeaderMap, Uri};
use r2e_security::guards::{RoleBasedIdentity, RolesGuard};

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

impl RoleBasedIdentity for TestIdentity {
    fn roles(&self) -> &[String] {
        &self.roles
    }
}

fn make_uri(s: &str) -> Uri {
    s.parse().unwrap()
}

fn make_ctx<'a, I: Identity>(
    identity: Option<&'a I>,
    uri: &'a Uri,
    headers: &'a HeaderMap,
) -> GuardContext<'a, I> {
    GuardContext {
        method_name: "test_method",
        controller_name: "TestController",
        headers,
        uri,
        path_params: PathParams::EMPTY,
        identity,
    }
}

#[tokio::test]
async fn roles_guard_passes() {
    let guard = RolesGuard {
        required_roles: &["admin"],
    };
    let id = TestIdentity::new("user-1", &["admin", "user"]);
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers);
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
    let ctx = make_ctx(Some(&id), &uri, &headers);
    let result = guard.check(&(), &ctx).await;
    assert!(result.is_err());
    let resp = result.unwrap_err();
    assert_eq!(resp.status(), r2e_core::http::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn roles_guard_rejects_no_identity() {
    let guard = RolesGuard {
        required_roles: &["admin"],
    };
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, TestIdentity> = make_ctx(None, &uri, &headers);
    let result = guard.check(&(), &ctx).await;
    assert!(result.is_err());
    let resp = result.unwrap_err();
    assert_eq!(resp.status(), r2e_core::http::StatusCode::FORBIDDEN);
}
