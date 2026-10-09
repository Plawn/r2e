use r2e_core::decorators::guards::{Guard, GuardContext, Identity, PathParams};
use r2e_core::http::{HeaderMap, Uri};
use r2e_security::guards::{AllRolesGuard, RoleBasedIdentity, RolesGuard};

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
        method: r2e_core::default_method(),
        extensions: r2e_core::no_extensions(),
        headers,
        uri,
        peer_addr: None,
        path_params: PathParams::EMPTY,
        identity,
    }
}

#[r2e_core::test]
async fn roles_guard_passes() {
    let guard = RolesGuard {
        required_roles: &["admin"],
    };
    let id = TestIdentity::new("user-1", &["admin", "user"]);
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers);
    let result = guard.check(&ctx).await;
    assert!(result.is_ok());
}

#[r2e_core::test]
async fn roles_guard_rejects() {
    let guard = RolesGuard {
        required_roles: &["admin"],
    };
    let id = TestIdentity::new("user-1", &["user"]);
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers);
    let result = guard.check(&ctx).await;
    assert!(result.is_err());
    let resp = result.unwrap_err();
    assert_eq!(resp.status(), r2e_core::http::StatusCode::FORBIDDEN);
}

#[r2e_core::test]
async fn roles_guard_rejects_no_identity() {
    let guard = RolesGuard {
        required_roles: &["admin"],
    };
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, TestIdentity> = make_ctx(None, &uri, &headers);
    let result = guard.check(&ctx).await;
    assert!(result.is_err());
    let resp = result.unwrap_err();
    assert_eq!(resp.status(), r2e_core::http::StatusCode::FORBIDDEN);
}

#[r2e_core::test]
async fn roles_guard_rejects_empty_requirements() {
    let user = TestIdentity {
        sub: "alice".into(),
        roles: vec!["admin".into()],
    };
    let guard = RolesGuard {
        required_roles: &[],
    };
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&user), &uri, &headers);

    assert!(guard.check(&ctx).await.is_err());
}

// ── AllRolesGuard (AND semantics) ──

#[r2e_core::test]
async fn all_roles_guard_passes_when_all_present() {
    let guard = AllRolesGuard {
        required_roles: &["admin", "editor"],
    };
    let id = TestIdentity::new("user-1", &["admin", "editor", "user"]);
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers);
    let result = guard.check(&ctx).await;
    assert!(result.is_ok());
}

#[r2e_core::test]
async fn all_roles_guard_rejects_when_one_missing() {
    let guard = AllRolesGuard {
        required_roles: &["admin", "superadmin"],
    };
    let id = TestIdentity::new("user-1", &["admin", "user"]);
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers);
    let result = guard.check(&ctx).await;
    assert!(result.is_err());
    let resp = result.unwrap_err();
    assert_eq!(resp.status(), r2e_core::http::StatusCode::FORBIDDEN);
}

#[r2e_core::test]
async fn all_roles_guard_rejects_no_identity() {
    let guard = AllRolesGuard {
        required_roles: &["admin"],
    };
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx: GuardContext<'_, TestIdentity> = make_ctx(None, &uri, &headers);
    let result = guard.check(&ctx).await;
    assert!(result.is_err());
    let resp = result.unwrap_err();
    assert_eq!(resp.status(), r2e_core::http::StatusCode::FORBIDDEN);
}

#[r2e_core::test]
async fn all_roles_guard_passes_single_role() {
    let guard = AllRolesGuard {
        required_roles: &["admin"],
    };
    let id = TestIdentity::new("user-1", &["admin"]);
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&id), &uri, &headers);
    let result = guard.check(&ctx).await;
    assert!(result.is_ok());
}

#[r2e_core::test]
async fn all_roles_guard_rejects_empty_requirements() {
    let user = TestIdentity {
        sub: "alice".into(),
        roles: vec!["admin".into()],
    };
    let guard = AllRolesGuard {
        required_roles: &[],
    };
    let uri = make_uri("/test");
    let headers = HeaderMap::new();
    let ctx = make_ctx(Some(&user), &uri, &headers);

    assert!(guard.check(&ctx).await.is_err());
}

// ---------------------------------------------------------------------------
// RolesDenied: the typed roles-guard error (error projection, #1072)
// ---------------------------------------------------------------------------

mod roles_denied {
    use http_body_util::BodyExt;
    use r2e_core::error::{Rejection, RejectionKind};
    use r2e_core::http::response::IntoResponse;
    use r2e_core::http::StatusCode;
    use r2e_security::RolesDenied;

    #[test]
    fn messages_match_the_legacy_bodies() {
        assert_eq!(
            RolesDenied::NoIdentity.message(),
            "No identity available for role check"
        );
        assert_eq!(RolesDenied::Insufficient.message(), "Insufficient roles");
        assert_eq!(RolesDenied::Insufficient.to_string(), "Insufficient roles");
    }

    #[test]
    fn both_variants_are_forbidden_rejections() {
        for denied in [RolesDenied::NoIdentity, RolesDenied::Insufficient] {
            let rejection = Rejection::from(denied);
            assert_eq!(rejection.kind, RejectionKind::Forbidden, "{denied:?}");
            assert_eq!(rejection.status, StatusCode::FORBIDDEN, "{denied:?}");
            assert_eq!(rejection.message, denied.message(), "{denied:?}");
            assert!(rejection.headers.is_empty(), "{denied:?}");
        }
    }

    #[r2e_core::test]
    async fn renders_the_same_response_as_the_rejection() {
        let resp = RolesDenied::Insufficient.into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({ "error": "Insufficient roles" }));
    }
}
