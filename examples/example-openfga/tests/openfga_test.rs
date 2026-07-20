//! Integration tests for the OpenFGA example.
//!
//! A real OpenFGA server is provided by `DevOpenFga` (testcontainers). The test
//! creates a store, writes the document model + seed tuples, then boots the
//! real app against that server and exercises the `FgaCheck` guards through
//! actual HTTP requests — one allowed, one denied.
//!
//! Requires Docker; `#[ignore]`d by default:
//!
//! ```bash
//! cargo test -p example-openfga --test openfga_test -- --ignored
//! ```

use example_openfga::{document_model, OpenFgaApp};
use r2e_devservices::DevOpenFga;
use r2e_test::TestApp;

/// Boot the app against a freshly bootstrapped OpenFGA store with these tuples:
/// - `user:alice` is a `viewer` and `editor` of `document:readme`
/// - `user:bob` has no relations
async fn boot() -> TestApp {
    let fga = DevOpenFga::shared().await;
    let store_id = fga.create_store("documents").await;
    let model_id = fga.write_model(&store_id, &document_model()).await;
    fga.write_tuples(
        &store_id,
        &model_id,
        &[
            ("user:alice", "viewer", "document:readme"),
            ("user:alice", "editor", "document:readme"),
        ],
    )
    .await;

    let grpc = fga.grpc_endpoint().to_string();
    TestApp::boot_with::<OpenFgaApp>(move |b| {
        b.override_config_value("openfga.endpoint", grpc)
            .override_config_value("openfga.store_id", store_id)
            .override_config_value("openfga.model_id", model_id)
    })
    .await
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
