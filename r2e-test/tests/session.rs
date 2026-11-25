// TestSession requires a router to work — these are unit-style tests for the
// public API surface. Integration tests that exercise actual HTTP flows
// would need an Axum router with cookie-setting endpoints.

use r2e_core::http::Routes;
use r2e_test::TestApp;

fn make_app() -> TestApp {
    // Minimal router with no routes — just to construct a TestApp
    TestApp::new(Routes::new())
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

// ─── Cookie carry-over through a live router ───

fn cookie_app() -> TestApp {
    use r2e_core::http::routing::get;
    use r2e_core::http::{Body, Request, Response};

    TestApp::new(
        Routes::new()
            .route(
                "/login",
                get(|_, _req: Request| async {
                    Response::builder()
                        .header("set-cookie", "sid=s1; Path=/; HttpOnly")
                        .header("set-cookie", "csrf=c1")
                        .body(Body::from("ok"))
                        .unwrap()
                }),
            )
            .route(
                "/echo",
                get(|_, req: Request| async move {
                    req.headers()
                        .get("cookie")
                        .map(|v| v.to_str().unwrap().to_owned())
                        .unwrap_or_else(|| "<none>".to_owned())
                }),
            ),
    )
}

#[r2e::test]
async fn test_session_sends_no_cookie_header_when_jar_is_empty() {
    let app = cookie_app();
    let session = app.session();
    assert_eq!(session.get("/echo").send().await.text(), "<none>");
}

#[r2e::test]
async fn test_session_sends_a_single_cookie_without_separator() {
    let app = cookie_app();
    let session = app.session();
    session.set_cookie("only", "v");
    assert_eq!(session.get("/echo").send().await.text(), "only=v");
}

#[r2e::test]
async fn test_session_captures_set_cookie_and_sends_them_back() {
    let app = cookie_app();
    let session = app.session();

    session.get("/login").send().await.assert_ok();
    assert_eq!(session.cookie("sid").as_deref(), Some("s1"));
    assert_eq!(session.cookie("csrf").as_deref(), Some("c1"));

    // The jar is ordered by name, `; `-separated.
    assert_eq!(session.get("/echo").send().await.text(), "csrf=c1; sid=s1");
}
