//! Blueprint-based integration tests: boot the **real** application through
//! `example_app::app` instead of re-declaring controllers and services.
//!
//! This file is the showcase for the Quarkus-grade testing DX:
//! - `#[r2e::test(app = ...)]`  → `@QuarkusTest` (boot the real app)
//! - `#[inject] bean: T`        → `@Inject` in the test class
//! - `.as_user("alice", ...)`   → `@TestSecurity` (no IdP needed)
//! - `with = |b| ...`           → `@InjectMock` / `@TestProfile` overrides
//! - `application-test.yaml`    → test profile config overlay

use example_app::models::User;
use example_app::services::UserService;
use r2e_test::{TestApp, TestJwt};

// ── Boot + HTTP ─────────────────────────────────────────────────────────

#[r2e::test(app = example_app::app)]
async fn health_endpoint(app: TestApp) {
    let resp = app.get("/health").send().await;
    resp.assert_ok();
    assert_eq!(resp.text(), "OK");
}

#[r2e::test(app = example_app::app)]
async fn anonymous_request_is_rejected(app: TestApp) {
    app.get("/users").send().await.assert_unauthorized();
}

// ── @TestSecurity: .as_user mints tokens accepted by the pinned validator ──

#[r2e::test(app = example_app::app)]
async fn list_users_as_authenticated_user(app: TestApp) {
    let resp = app.get("/users").as_user("user-1", &["user"]).send().await;
    resp.assert_ok();
    let users: Vec<User> = resp.json();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0].name, "Alice");
}

#[r2e::test(app = example_app::app)]
async fn admin_endpoint_enforces_roles(app: TestApp) {
    app.get("/mixed/admin")
        .as_user("user-1", &["user"])
        .send()
        .await
        .assert_forbidden();

    app.get("/mixed/admin")
        .as_user("admin-1", &["admin"])
        .send()
        .await
        .assert_ok();
}

// ── Fail-closed auth with #[anonymous] opt-out (ReportController) ────────

#[r2e::test(app = example_app::app)]
async fn anonymous_route_is_public(app: TestApp) {
    // No credentials at all — the marked route skips identity extraction.
    app.get("/reports/summary").send().await.assert_ok();
}

#[r2e::test(app = example_app::app)]
async fn unmarked_routes_are_authenticated_by_default(app: TestApp) {
    app.get("/reports/full").send().await.assert_unauthorized();

    app.get("/reports/full")
        .as_user("user-1", &["user"])
        .send()
        .await
        .assert_ok();
}

#[r2e::test(app = example_app::app)]
async fn struct_identity_feeds_roles_without_params(app: TestApp) {
    // Anonymous → 401 from identity extraction (fail-closed default).
    app.get("/reports/audit").send().await.assert_unauthorized();

    // Authenticated without the role → 403 from #[roles("admin")].
    app.get("/reports/audit")
        .as_user("user-1", &["user"])
        .send()
        .await
        .assert_forbidden();

    app.get("/reports/audit")
        .as_user("admin-1", &["admin"])
        .send()
        .await
        .assert_ok();
}

#[r2e::test(app = example_app::app)]
async fn negative_token_paths(app: TestApp, jwt: TestJwt) {
    let bad = jwt.wrong_issuer_token("user-1");
    app.get("/users").bearer(&bad).send().await.assert_unauthorized();

    app.get("/users")
        .bearer(&TestJwt::malformed_token())
        .send()
        .await
        .assert_unauthorized();
}

// ── @Inject: beans from the real app's graph, injected into the test ────

#[r2e::test(app = example_app::app)]
async fn beans_are_injectable_into_tests(app: TestApp, #[inject] users: UserService) {
    assert_eq!(users.count().await, 2);

    app.post("/users")
        .as_user("user-1", &["user"])
        .json(&serde_json::json!({ "name": "Charlie", "email": "charlie@example.com" }))
        .send()
        .await
        .assert_ok();

    // The injected bean is the same instance the controller used.
    assert_eq!(users.count().await, 3);
}

// ── @TestProfile: application-test.yaml + per-test config overrides ─────

#[r2e::test(app = example_app::app)]
async fn test_profile_overlays_config(app: TestApp) {
    // application.yaml says "Welcome to R2E!"; application-test.yaml
    // overrides app.greeting under the forced `test` profile.
    let resp = app.get("/users/greet").as_user("user-1", &["user"]).send().await;
    resp.assert_ok();
    assert_eq!(resp.text(), "Hello from the test profile!");

    assert_eq!(
        app.config().get::<String>("app.greeting").unwrap(),
        "Hello from the test profile!"
    );
}

#[r2e::test(app = example_app::app, with = |b| b.override_config_value("app.greeting", "patched for this test"))]
async fn config_keys_can_be_patched_per_test(app: TestApp) {
    let resp = app.get("/users/greet").as_user("user-1", &["user"]).send().await;
    resp.assert_ok();
    assert_eq!(resp.text(), "patched for this test");
}

// ── @InjectMock: pin a replacement bean over the app's own registration ──

#[r2e::test(app = example_app::app, with = |b| {
    let bus = r2e::r2e_events::LocalEventBus::new();
    b.override_bean(example_app::services::UserService::new(bus))
})]
async fn beans_can_be_pinned_over_the_apps_own(app: TestApp, #[inject] users: UserService) {
    // The pinned instance replaced the module-provided one: the controller
    // and the injected bean both see it.
    app.post("/users")
        .as_user("user-1", &["user"])
        .json(&serde_json::json!({ "name": "Dave", "email": "dave@example.com" }))
        .send()
        .await
        .assert_ok();
    assert_eq!(users.count().await, 3);
}
