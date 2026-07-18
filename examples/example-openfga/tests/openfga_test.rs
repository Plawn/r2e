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
    app.get("/documents/readme").send().await.assert_unauthorized();
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
