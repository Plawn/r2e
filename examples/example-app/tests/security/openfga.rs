//! OpenFGA fine-grained authorization, exercised against the real app.
//!
//! The app seeds an in-memory FGA backend (see `app.rs`): `alice` is a
//! viewer+editor of `document:readme` and a viewer-only of `document:roadmap`.
//! These tests prove one allowed and one denied request per relation, plus the
//! fail-closed behaviour for anonymous callers.

use example_app::controllers::document_controller::Document;
use r2e_test::TestApp;

#[r2e::test(app = example_app::ExampleApp)]
async fn viewer_can_read_document(app: TestApp) {
    // alice is a `viewer` of document:readme → 200.
    let resp = app
        .get("/documents/readme")
        .as_user("alice", &["user"])
        .send()
        .await;
    resp.assert_ok();
    let doc: Document = resp.json();
    assert_eq!(doc.id, "readme");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn non_viewer_is_denied(app: TestApp) {
    // alice has no relation to document:secret → 403.
    app.get("/documents/secret")
        .as_user("alice", &["user"])
        .send()
        .await
        .assert_forbidden();
}

#[r2e::test(app = example_app::ExampleApp)]
async fn editor_can_update_document(app: TestApp) {
    // alice is an `editor` of document:readme → 200.
    let resp = app
        .put("/documents/readme")
        .as_user("alice", &["user"])
        .json(&serde_json::json!({ "body": "rewritten by alice" }))
        .send()
        .await;
    resp.assert_ok();
    let doc: Document = resp.json();
    assert_eq!(doc.body, "rewritten by alice");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn viewer_without_editor_cannot_update(app: TestApp) {
    // alice is only a `viewer` of document:roadmap, not an `editor` → 403.
    app.put("/documents/roadmap")
        .as_user("alice", &["user"])
        .json(&serde_json::json!({ "body": "should be rejected" }))
        .send()
        .await
        .assert_forbidden();
}

#[r2e::test(app = example_app::ExampleApp)]
async fn anonymous_caller_is_unauthorized(app: TestApp) {
    // Struct-level identity authenticates every route: no token → 401,
    // before any FGA check runs.
    app.get("/documents/readme")
        .send()
        .await
        .assert_unauthorized();
}
