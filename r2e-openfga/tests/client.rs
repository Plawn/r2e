//! `FgaClient` — typed grant/revoke/check over a `MockBackend`, including
//! write-through cache invalidation.
//!
//! The `authz` module below hand-writes exactly what `model!` generates
//! (`model!` itself cannot expand inside r2e-openfga's own tests — the
//! proc-macro resolves the runtime crate to `crate::`, which points at the
//! test crate here; macro coverage lives in example-openfga and
//! r2e-compile-tests). Model mirrored:
//!
//! ```text
//! type user
//! type team
//!   relations
//!     define member: [user]
//! type document
//!   relations
//!     define viewer: [user, team#member, user:*]
//!     define editor: [user]
//! ```

use r2e_openfga::{FgaClient, MockBackend, OpenFgaError, OpenFgaRegistry};

mod authz {
    pub mod user {
        use r2e_openfga::typed::*;
        pub struct Ty;
        impl FgaType for Ty {
            const NAME: &'static str = "user";
        }
        pub fn id(id: impl AsRef<str>) -> FgaObject<Ty> {
            FgaObject::new(id)
        }
        pub fn wildcard() -> FgaWildcard<Ty> {
            FgaWildcard::new()
        }
    }

    pub mod team {
        use r2e_openfga::typed::*;
        pub struct Ty;
        impl FgaType for Ty {
            const NAME: &'static str = "team";
        }
        pub fn id(id: impl AsRef<str>) -> FgaObject<Ty> {
            FgaObject::new(id)
        }
        pub struct Member;
        #[allow(non_upper_case_globals)]
        pub const member: FgaRel<Ty, Member> = FgaRel::new("member");
        impl DirectlyAssignable<super::user::Ty> for Member {}
    }

    pub mod document {
        use r2e_openfga::typed::*;
        pub struct Ty;
        impl FgaType for Ty {
            const NAME: &'static str = "document";
        }
        pub fn id(id: impl AsRef<str>) -> FgaObject<Ty> {
            FgaObject::new(id)
        }
        pub struct Viewer;
        #[allow(non_upper_case_globals)]
        pub const viewer: FgaRel<Ty, Viewer> = FgaRel::new("viewer");
        impl DirectlyAssignable<super::user::Ty> for Viewer {}
        impl DirectlyAssignable<(super::team::Ty, super::team::Member)> for Viewer {}
        impl DirectlyAssignable<WildcardOf<super::user::Ty>> for Viewer {}

        pub struct Editor;
        #[allow(non_upper_case_globals)]
        pub const editor: FgaRel<Ty, Editor> = FgaRel::new("editor");
        impl DirectlyAssignable<super::user::Ty> for Editor {}
    }
}

fn client_with_mock(cache_ttl_secs: Option<u64>) -> (FgaClient, MockBackend) {
    let mock = MockBackend::new();
    let registry = match cache_ttl_secs {
        Some(ttl) => OpenFgaRegistry::with_cache(mock.clone(), ttl),
        None => OpenFgaRegistry::new(mock.clone()),
    };
    (FgaClient::new(registry), mock)
}

#[tokio::test]
async fn grant_writes_tuple_and_check_sees_it() {
    let (fga, mock) = client_with_mock(None);
    let alice = authz::user::id("alice");
    let doc = authz::document::id("readme");

    fga.grant(&alice, authz::document::viewer, &doc)
        .await
        .unwrap();

    assert!(mock.has_tuple("user:alice", "viewer", "document:readme"));
    assert!(fga
        .check(&alice, authz::document::viewer, &doc)
        .await
        .unwrap());
}

#[tokio::test]
async fn revoke_deletes_tuple() {
    let (fga, mock) = client_with_mock(None);
    let alice = authz::user::id("alice");
    let doc = authz::document::id("readme");

    fga.grant(&alice, authz::document::viewer, &doc)
        .await
        .unwrap();
    fga.revoke(&alice, authz::document::viewer, &doc)
        .await
        .unwrap();

    assert!(!mock.has_tuple("user:alice", "viewer", "document:readme"));
    assert!(!fga
        .check(&alice, authz::document::viewer, &doc)
        .await
        .unwrap());
}

/// The write-through contract: a cached *deny* must not survive a grant,
/// and a cached *allow* must not survive a revoke — with a TTL long enough
/// that expiry cannot be the explanation.
#[tokio::test]
async fn grant_and_revoke_invalidate_cached_decisions() {
    let (fga, _mock) = client_with_mock(Some(3600));
    let alice = authz::user::id("alice");
    let doc = authz::document::id("readme");

    // Prime the cache with a deny.
    assert!(!fga
        .check(&alice, authz::document::viewer, &doc)
        .await
        .unwrap());

    fga.grant(&alice, authz::document::viewer, &doc)
        .await
        .unwrap();
    assert!(
        fga.check(&alice, authz::document::viewer, &doc)
            .await
            .unwrap(),
        "grant must invalidate the cached deny"
    );

    fga.revoke(&alice, authz::document::viewer, &doc)
        .await
        .unwrap();
    assert!(
        !fga.check(&alice, authz::document::viewer, &doc)
            .await
            .unwrap(),
        "revoke must invalidate the cached allow"
    );
}

#[tokio::test]
async fn userset_and_wildcard_subjects_render_wire_form() {
    let (fga, mock) = client_with_mock(None);
    let doc = authz::document::id("readme");

    let eng_members = authz::team::member.of(authz::team::id("eng"));
    fga.grant(&eng_members, authz::document::viewer, &doc)
        .await
        .unwrap();
    assert!(mock.has_tuple("team:eng#member", "viewer", "document:readme"));

    fga.grant(&authz::user::wildcard(), authz::document::viewer, &doc)
        .await
        .unwrap();
    assert!(mock.has_tuple("user:*", "viewer", "document:readme"));
}

/// A check-only custom backend still compiles; the tuple operations
/// surface `Unsupported` instead of silently doing nothing.
#[tokio::test]
async fn check_only_backend_reports_unsupported_writes() {
    use r2e_openfga::OpenFgaBackend;
    use std::future::Future;
    use std::pin::Pin;

    struct CheckOnly;
    impl OpenFgaBackend for CheckOnly {
        fn check(
            &self,
            _user: &str,
            _relation: &str,
            _object: &str,
        ) -> Pin<Box<dyn Future<Output = Result<bool, OpenFgaError>> + Send + '_>> {
            Box::pin(async { Ok(true) })
        }
    }

    let fga = FgaClient::new(OpenFgaRegistry::new(CheckOnly));
    let alice = authz::user::id("alice");
    let doc = authz::document::id("readme");

    let err = fga
        .grant(&alice, authz::document::viewer, &doc)
        .await
        .unwrap_err();
    assert!(matches!(err, OpenFgaError::Unsupported("write_tuple")));
    let err = fga
        .revoke(&alice, authz::document::viewer, &doc)
        .await
        .unwrap_err();
    assert!(matches!(err, OpenFgaError::Unsupported("delete_tuple")));
}
