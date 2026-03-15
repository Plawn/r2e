// TestSession requires a router to work — these are unit-style tests for the
// public API surface. Integration tests that exercise actual HTTP flows
// would need an Axum router with cookie-setting endpoints.

use r2e_test::TestApp;
use r2e_core::http::Router;

fn make_app() -> TestApp {
    // Minimal router with no routes — just to construct a TestApp
    TestApp::new(Router::new())
}

#[test]
fn test_session_cookie_management() {
    let app = make_app();
    let session = app.session();

    // Initially empty
    assert!(session.cookie("token").is_none());

    // Set a cookie
    session.set_cookie("token", "abc123");
    assert_eq!(session.cookie("token"), Some("abc123".to_string()));

    // Overwrite
    session.set_cookie("token", "xyz789");
    assert_eq!(session.cookie("token"), Some("xyz789".to_string()));

    // Remove
    session.remove_cookie("token");
    assert!(session.cookie("token").is_none());
}

#[test]
fn test_session_clear_cookies() {
    let app = make_app();
    let session = app.session();

    session.set_cookie("a", "1");
    session.set_cookie("b", "2");

    session.clear_cookies();
    assert!(session.cookie("a").is_none());
    assert!(session.cookie("b").is_none());
}

#[test]
fn test_session_builder_pattern() {
    let app = make_app();
    // Verify the builder pattern compiles and doesn't panic
    let _session = app
        .session()
        .with_bearer("test-token")
        .with_default_header("x-custom", "value");
}
