//! Integration tests for the OpenFGA example.
//!
//! A real OpenFGA server is provided by `DevOpenFga` (testcontainers). The
//! `OpenFga` plugin owns the store lifecycle: pointed at the dev server, it
//! creates the (per-test) store and applies `authz::MODEL` at boot — the test
//! only seeds tuples through the typed `FgaClient` bean, then exercises the
//! `FgaCheck` guards through actual HTTP requests — one allowed, one denied.
//!
//! Requires Docker; `#[ignore]`d by default:
//!
//! ```bash
//! cargo test -p example-openfga --test openfga_test -- --ignored
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

use example_openfga::{authz, OpenFgaApp};
use r2e::r2e_openfga::FgaClient;
use r2e_devservices::DevOpenFga;
use r2e_test::TestApp;

/// Boot the app against the shared dev OpenFGA server. The plugin creates a
/// unique store per test (isolation on the session-shared container) and
/// applies the model; the seed tuples make:
/// - `user:alice` a `viewer` and `editor` of `document:readme`
/// - `user:bob` nothing
async fn boot() -> TestApp {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let fga = DevOpenFga::shared().await;
    let grpc = fga.grpc_endpoint().to_string();
    let store = format!(
        "documents-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );

    let app = TestApp::boot_with::<OpenFgaApp>(move |b| {
        b.override_config_value("openfga.endpoint", grpc)
            .override_config_value("openfga.store", store)
    })
    .await;

    let client = app.bean::<FgaClient>();
    let alice = authz::user::id("alice");
    let readme = authz::document::id("readme");
    client
        .grant(&alice, authz::document::viewer, &readme)
        .await
        .expect("seed viewer");
    client
        .grant(&alice, authz::document::editor, &readme)
        .await
        .expect("seed editor");

    app
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn unauthenticated_request_is_rejected() {
    let app = boot().await;
    app.get("/documents/readme")
        .send()
        .await
        .assert_unauthorized();
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn viewer_is_allowed_to_read() {
    let app = boot().await;
    let resp = app
        .get("/documents/readme")
        .as_user("alice", &[])
        .send()
        .await;
    resp.assert_ok();
    let body: serde_json::Value = resp.json();
    assert_eq!(body["action"], "view");
    assert_eq!(body["user"], "alice");
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn non_viewer_is_denied() {
    let app = boot().await;
    app.get("/documents/readme")
        .as_user("bob", &[])
        .send()
        .await
        .assert_forbidden();
}

/// The typed write path end-to-end: an editor shares the document (grant
/// through `FgaClient`), the grantee immediately gains access (write-through
/// cache invalidation), then loses it again on unshare.
#[tokio::test]
#[ignore = "requires Docker"]
async fn share_grants_and_unshare_revokes_view_access() {
    let app = boot().await;

    // Prime the decision cache with bob's deny.
    app.get("/documents/readme")
        .as_user("bob", &[])
        .send()
        .await
        .assert_forbidden();

    // Alice (editor) shares with bob; bob's next request must see it even
    // though his deny was cached.
    app.post("/documents/readme/share/bob")
        .as_user("alice", &[])
        .send()
        .await
        .assert_ok();
    app.get("/documents/readme")
        .as_user("bob", &[])
        .send()
        .await
        .assert_ok();

    app.delete("/documents/readme/share/bob")
        .as_user("alice", &[])
        .send()
        .await
        .assert_ok();
    app.get("/documents/readme")
        .as_user("bob", &[])
        .send()
        .await
        .assert_forbidden();
}

/// Sharing is editor-gated: a mere viewer cannot grant access.
#[tokio::test]
#[ignore = "requires Docker"]
async fn non_editor_cannot_share() {
    let app = boot().await;

    // Alice grants bob viewer only.
    app.post("/documents/readme/share/bob")
        .as_user("alice", &[])
        .send()
        .await
        .assert_ok();

    app.post("/documents/readme/share/carol")
        .as_user("bob", &[])
        .send()
        .await
        .assert_forbidden();
}

/// Metacharacters in the grantee id are rejected before reaching the store —
/// the same `:`/`#`/`*` guard as the request-time object resolvers.
#[tokio::test]
#[ignore = "requires Docker"]
async fn share_rejects_metacharacters_in_user_id() {
    let app = boot().await;

    app.post("/documents/readme/share/bob%23admin")
        .as_user("alice", &[])
        .send()
        .await
        .assert_bad_request();
}

#[tokio::test]
#[ignore = "requires Docker"]
async fn editor_is_allowed_but_non_editor_is_denied() {
    let app = boot().await;

    app.put("/documents/readme")
        .as_user("alice", &[])
        .send()
        .await
        .assert_ok();

    app.put("/documents/readme")
        .as_user("bob", &[])
        .send()
        .await
        .assert_forbidden();
}
