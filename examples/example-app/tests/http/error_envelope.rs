//! Error envelopes end to end (0.5 error projection), on the real demo app.
//!
//! `ProblemController` returns `Result<T, Problem>` on two routes: every
//! failure there — identity, content-type, body, path, garde, the handler's own
//! `Err` — renders as one `application/problem+json` shape. Its infallible
//! route, 404s and the OpenAPI spec exercise the app-level envelope installed
//! with `.error_projection::<AppError>()`.

use r2e_test::TestApp;
use serde_json::Value;

const PROBLEM: &str = "application/problem+json";

fn refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "$ref" {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                    }
                } else {
                    refs(v, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| refs(v, out)),
        _ => {}
    }
}

fn refs_of(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    refs(v, &mut out);
    out
}

// ── Result<T, Problem> routes ─────────────────────────────────────────────

#[r2e::test(app = example_app::ExampleApp)]
async fn missing_token_is_a_problem_and_the_body_is_never_read(app: TestApp) {
    let resp = app
        .post("/problems/")
        .content_type("application/json")
        .body("{not even json")
        .send()
        .await;
    resp.assert_unauthorized();
    assert_eq!(resp.header("content-type"), Some(PROBLEM));
    let body: Value = resp.json();
    assert_eq!(body["status"], 401);
    assert_eq!(body["title"], "Unauthorized");
    assert_eq!(body["code"], "Unauthenticated");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn missing_content_type_is_a_415_problem(app: TestApp) {
    let resp = app
        .post("/problems/")
        .as_user("alice", &["user"])
        .body(r#"{"title":"Printer on fire"}"#)
        .send()
        .await;
    resp.assert_status(r2e::http::StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(resp.header("content-type"), Some(PROBLEM));
    let body: Value = resp.json();
    assert_eq!(body["status"], 415);
    assert_eq!(body["code"], "MissingContentType");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn malformed_json_is_a_400_problem(app: TestApp) {
    let resp = app
        .post("/problems/")
        .as_user("alice", &["user"])
        .content_type("application/json")
        .body("{not even json")
        .send()
        .await;
    resp.assert_bad_request();
    assert_eq!(resp.header("content-type"), Some(PROBLEM));
    let body: Value = resp.json();
    assert_eq!(body["status"], 400);
    assert_eq!(body["code"], "MalformedBody");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn garde_report_is_remapped_to_a_422_problem(app: TestApp) {
    let resp = app
        .post("/problems/")
        .as_user("alice", &["user"])
        .json(&serde_json::json!({ "title": "no" }))
        .send()
        .await;
    // `Problem::status_of(Validation)` says 422 where `HttpError` says 400.
    resp.assert_unprocessable();
    assert_eq!(resp.header("content-type"), Some(PROBLEM));
    let body: Value = resp.json();
    assert_eq!(body["status"], 422);
    assert_eq!(body["code"], "Validation");
    let errors = body["errors"].as_array().expect("garde field errors carried in `errors`");
    assert!(errors.iter().any(|e| e["field"] == "title"), "{errors:?}");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn valid_request_creates_the_ticket(app: TestApp) {
    let resp = app
        .post("/problems/")
        .as_user("alice", &["user"])
        .json(&serde_json::json!({ "title": "Printer on fire" }))
        .send()
        .await;
    resp.assert_ok();
    let body: Value = resp.json();
    assert_eq!(body["title"], "Printer on fire");
    assert_eq!(body["reporter"], "alice");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn bad_path_segment_is_a_400_problem(app: TestApp) {
    let resp = app.get("/problems/not-a-number").send().await;
    resp.assert_bad_request();
    assert_eq!(resp.header("content-type"), Some(PROBLEM));
    let body: Value = resp.json();
    assert_eq!(body["code"], "InvalidPath");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn handler_err_is_the_same_problem_shape(app: TestApp) {
    let resp = app.get("/problems/99").send().await;
    resp.assert_not_found();
    assert_eq!(resp.header("content-type"), Some(PROBLEM));
    let body: Value = resp.json();
    assert_eq!(body["status"], 404);
    assert_eq!(body["title"], "Not Found");
    assert_eq!(body["detail"], "No ticket #99");
    assert!(body.get("code").is_none(), "a handler-built Problem has no rejection code");
}

// ── App-level envelope (`.error_projection::<AppError>()`) ───────────────

#[r2e::test(app = example_app::ExampleApp)]
async fn infallible_route_uses_the_app_level_envelope(app: TestApp) {
    let resp = app.get("/problems/legacy/not-a-number").send().await;
    resp.assert_bad_request();
    assert_eq!(resp.header("content-type"), Some("application/json"));
    let body: Value = resp.json();
    assert!(body["error"].is_string(), "plain `{{\"error\": ..}}` body, got {body}");
    assert!(body.get("status").is_none(), "not a Problem");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn wrong_method_renders_through_the_app_level_envelope(app: TestApp) {
    // 405 is the framework's own response (`RejectionKind::MethodNotAllowed`),
    // rendered with the app-level envelope. (404 is not testable here: the demo
    // app installs its own `#[fallback]` in `ProxyController`.)
    let resp = app.delete("/problems/1").send().await;
    resp.assert_status(r2e::http::StatusCode::METHOD_NOT_ALLOWED);
    assert!(resp.header("allow").is_some(), "`Allow` is kept");
    let body: Value = resp.json();
    assert_eq!(body, serde_json::json!({ "error": "Method not allowed" }));
}

// ── OpenAPI: the spec reads the same `status_of` table ───────────────────

#[r2e::test(app = example_app::ExampleApp)]
async fn openapi_documents_the_problem_envelope_per_route(app: TestApp) {
    let spec: Value = app.get("/openapi.json").send().await.json();

    let create = &spec["paths"]["/problems/"]["post"]["responses"];
    for status in ["400", "401", "413", "415", "422", "500"] {
        assert!(create.get(status).is_some(), "POST /problems/ documents {status}: {create}");
    }
    // garde → 422 (remapped), never a 400 validation body.
    let v422 = refs_of(&create["422"]);
    assert_eq!(v422, vec!["#/components/schemas/Problem".to_string()], "{create}");
    // 500 = the route's own `Problem` + the app envelope's panic body (`anyOf`).
    let v500 = refs_of(&create["500"]);
    assert!(v500.contains(&"#/components/schemas/Problem".to_string()), "{v500:?}");
    assert!(v500.contains(&"#/components/schemas/ErrorResponse".to_string()), "{v500:?}");

    let legacy = &spec["paths"]["/problems/legacy/{id}"]["get"]["responses"];
    assert_eq!(
        refs_of(&legacy["400"]),
        vec!["#/components/schemas/ErrorResponse".to_string()],
        "infallible route is documented with the app-level envelope: {legacy}"
    );

    let problem = &spec["components"]["schemas"]["Problem"];
    assert!(problem.is_object(), "Problem component present");
    let required = problem["required"].as_array().expect("required list");
    assert!(required.iter().any(|r| r == "status"));
}
