use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use r2e_core::builder::AppBuilder;
use r2e_core::plugins::{Cors, DevReload, ErrorHandling, Health, NormalizePath};
use r2e_core::request_id::RequestIdPlugin;
use r2e_core::secure_headers::SecureHeaders;
use tower::ServiceExt;

async fn send_get(router: axum::Router, path: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).to_string())
}

async fn send_get_with_header(
    router: axum::Router,
    path: &str,
    header_name: &str,
    header_value: &str,
) -> axum::http::Response<Body> {
    let req = Request::builder()
        .uri(path)
        .header(header_name, header_value)
        .body(Body::empty())
        .unwrap();
    router.oneshot(req).await.unwrap()
}

async fn send_request(
    router: axum::Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> axum::http::Response<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let req = builder.body(Body::empty()).unwrap();
    router.oneshot(req).await.unwrap()
}

fn build_app() -> AppBuilder<()> {
    AppBuilder::new().with_state(())
}

// ── Health plugin ────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_200_ok() {
    let router = build_app().with(Health).build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── ErrorHandling plugin ────────────────────────────────────────────────

#[tokio::test]
async fn error_handling_catches_panic() {
    use axum::routing::get;

    let app = AppBuilder::new()
        .with_state(())
        .register_routes(axum::Router::new().route(
            "/panic",
            get(|| async {
                panic!("boom");
                #[allow(unreachable_code)]
                "never"
            }),
        ))
        .with(ErrorHandling)
        .build();

    let (status, body) = send_get(app, "/panic").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["error"], "Internal server error");
}

// ── NormalizePath plugin ────────────────────────────────────────────────

#[tokio::test]
async fn normalize_path_strips_trailing() {
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, body) = send_get(router, "/health/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── DevReload plugin ────────────────────────────────────────────────────

#[tokio::test]
async fn dev_reload_status() {
    let router = build_app().with(DevReload).build();
    let (status, body) = send_get(router, "/__r2e_dev/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "dev");
}

#[tokio::test]
async fn dev_reload_ping() {
    let router = build_app().with(DevReload).build();
    let (status, body) = send_get(router, "/__r2e_dev/ping").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["boot_time"].is_number());
    assert_eq!(json["status"], "ok");
}

// ── RequestIdPlugin ─────────────────────────────────────────────────────

#[tokio::test]
async fn request_id_generated() {
    let router = build_app().with(Health).with(RequestIdPlugin).build();
    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp.headers().get("x-request-id");
    assert!(req_id.is_some(), "response should have X-Request-Id header");
    let val = req_id.unwrap().to_str().unwrap();
    assert!(!val.is_empty());
}

#[tokio::test]
async fn request_id_propagated() {
    let router = build_app().with(Health).with(RequestIdPlugin).build();
    let resp = send_get_with_header(router, "/health", "x-request-id", "test-123").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp.headers().get("x-request-id").unwrap().to_str().unwrap();
    assert_eq!(req_id, "test-123");
}

// ── SecureHeaders plugin ────────────────────────────────────────────────

#[tokio::test]
async fn secure_headers_in_response() {
    let router = build_app()
        .with(Health)
        .with(SecureHeaders::default())
        .build();
    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let headers = resp.headers();
    assert_eq!(
        headers.get("x-content-type-options").unwrap().to_str().unwrap(),
        "nosniff"
    );
    assert_eq!(
        headers.get("x-frame-options").unwrap().to_str().unwrap(),
        "DENY"
    );
    assert!(headers.get("strict-transport-security").is_some());
    assert_eq!(
        headers.get("x-xss-protection").unwrap().to_str().unwrap(),
        "0"
    );
    assert!(headers.get("referrer-policy").is_some());
}

// ── Advanced Health ─────────────────────────────────────────────────────

use r2e_core::health::{HealthIndicator, HealthStatus};

struct AlwaysUp;
impl HealthIndicator for AlwaysUp {
    fn name(&self) -> &str {
        "always-up"
    }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        async { HealthStatus::Up }
    }
}

struct AlwaysDown;
impl HealthIndicator for AlwaysDown {
    fn name(&self) -> &str {
        "always-down"
    }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        async { HealthStatus::Down("intentionally down".into()) }
    }
}

struct LivenessOnly;
impl HealthIndicator for LivenessOnly {
    fn name(&self) -> &str {
        "liveness-only"
    }
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send {
        async { HealthStatus::Down("down but liveness-only".into()) }
    }
    fn affects_readiness(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn advanced_health_all_up() {
    let router = build_app()
        .with(Health::builder().check(AlwaysUp).build())
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
}

#[tokio::test]
async fn advanced_health_one_down() {
    let router = build_app()
        .with(Health::builder().check(AlwaysUp).check(AlwaysDown).build())
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "DOWN");
}

#[tokio::test]
async fn advanced_health_liveness_always_ok() {
    let router = build_app()
        .with(Health::builder().check(AlwaysDown).build())
        .build();
    let (status, _body) = send_get(router, "/health/live").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn advanced_health_readiness_filters() {
    // LivenessOnly check is down but doesn't affect readiness
    let router = build_app()
        .with(Health::builder().check(AlwaysUp).check(LivenessOnly).build())
        .build();
    let (status, body) = send_get(router, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
}

// ── E.1 CORS Plugin ──────────────────────────────────────────────────────

#[tokio::test]
async fn cors_permissive_allows_origin() {
    let router = build_app()
        .with(Health)
        .with(Cors::permissive())
        .build();
    let resp = send_get_with_header(router, "/health", "origin", "http://example.com").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("access-control-allow-origin").is_some(),
        "response should have access-control-allow-origin header"
    );
}

#[tokio::test]
async fn cors_preflight_returns_200() {
    let router = build_app()
        .with(Health)
        .with(Cors::permissive())
        .build();
    let resp = send_request(
        router,
        "OPTIONS",
        "/health",
        &[
            ("origin", "http://example.com"),
            ("access-control-request-method", "GET"),
        ],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("access-control-allow-origin").is_some());
    assert!(resp.headers().get("access-control-allow-methods").is_some());
}

// ── E.2 AppBuilder State Building ─────────────────────────────────────────

use r2e_core::beans::{AsyncBean, Bean, BeanContext, BeanState, Producer};
use r2e_core::type_list::{BuildableFrom, Contains};
use std::any::TypeId;

#[derive(Clone, Debug)]
struct TestDep(i32);

// State with a single dependency
#[derive(Clone)]
struct SingleDepState {
    dep: TestDep,
}

impl BeanState for SingleDepState {
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            dep: ctx.get::<TestDep>(),
        }
    }
}

impl<P, I0> BuildableFrom<P, (I0,)> for SingleDepState
where
    P: Contains<TestDep, I0>,
{
}

#[tokio::test]
async fn build_state_with_provide() {
    let router = AppBuilder::new()
        .provide(TestDep(42))
        .build_state::<SingleDepState, _>()
        .await
        .with(Health)
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// Bean test types
#[derive(Clone)]
struct TestService {
    label: String,
}

impl Bean for TestService {
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        TestService {
            label: "built".into(),
        }
    }
}

#[derive(Clone)]
struct BeanTestState {
    service: TestService,
}

impl BeanState for BeanTestState {
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            service: ctx.get::<TestService>(),
        }
    }
}

impl<P, I0> BuildableFrom<P, (I0,)> for BeanTestState
where
    P: Contains<TestService, I0>,
{
}

#[tokio::test]
async fn build_state_with_bean() {
    let router = AppBuilder::new()
        .with_bean::<TestService>()
        .build_state::<BeanTestState, _>()
        .await
        .with(Health)
        .build();
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
}

// Async bean
#[derive(Clone)]
struct AsyncService {
    label: String,
}

impl AsyncBean for AsyncService {
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> impl std::future::Future<Output = Self> + Send + '_ {
        async { AsyncService { label: "async-built".into() } }
    }
}

#[derive(Clone)]
struct AsyncBeanTestState {
    service: AsyncService,
}

impl BeanState for AsyncBeanTestState {
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            service: ctx.get::<AsyncService>(),
        }
    }
}

impl<P, I0> BuildableFrom<P, (I0,)> for AsyncBeanTestState
where
    P: Contains<AsyncService, I0>,
{
}

#[tokio::test]
async fn build_state_with_async_bean() {
    let router = AppBuilder::new()
        .with_async_bean::<AsyncService>()
        .build_state::<AsyncBeanTestState, _>()
        .await
        .with(Health)
        .build();
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
}

// Producer
#[derive(Clone)]
struct ProducedValue(String);

struct TestProducer;

impl Producer for TestProducer {
    type Output = ProducedValue;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn produce(_ctx: &BeanContext) -> impl std::future::Future<Output = Self::Output> + Send + '_ {
        async { ProducedValue("produced".into()) }
    }
}

#[derive(Clone)]
struct ProducerTestState {
    value: ProducedValue,
}

impl BeanState for ProducerTestState {
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            value: ctx.get::<ProducedValue>(),
        }
    }
}

impl<P, I0> BuildableFrom<P, (I0,)> for ProducerTestState
where
    P: Contains<ProducedValue, I0>,
{
}

#[tokio::test]
async fn build_state_with_producer() {
    let router = AppBuilder::new()
        .with_producer::<TestProducer>()
        .build_state::<ProducerTestState, _>()
        .await
        .with(Health)
        .build();
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn build_state_with_config_injection() {
    use r2e_core::config::{ConfigValue, R2eConfig};

    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::String("test-app".into()));
    // Provide both R2eConfig (for config injection) and TestDep (for state)
    let router = AppBuilder::new()
        .provide(config)
        .provide(TestDep(99))
        .build_state::<SingleDepState, _>()
        .await
        .with(Health)
        .build();
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
}

// ── E.3 Plugin Lifecycle ──────────────────────────────────────────────────

#[tokio::test]
async fn startup_hook_registration_accepted() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let flag = Arc::new(AtomicBool::new(false));
    let flag_clone = flag.clone();

    // Verify on_start hook registration doesn't panic.
    // Hooks only run in serve(), so we just verify build() succeeds.
    let router = build_app()
        .with(Health)
        .on_start(move |_state| async move {
            flag_clone.store(true, Ordering::SeqCst);
            Ok(())
        })
        .build();
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    // Hook was NOT called (only runs in serve()), but registration succeeded.
    assert!(!flag.load(Ordering::SeqCst));
}

#[tokio::test]
async fn plugin_ordering_layers_respected() {
    use axum::http::HeaderValue;

    let router = build_app()
        .with(Health)
        .with_layer_fn(|router| {
            router.layer(axum::middleware::from_fn(|req, next: axum::middleware::Next| async move {
                let mut resp = next.run(req).await;
                resp.headers_mut().insert("x-plugin-a", HeaderValue::from_static("a"));
                resp
            }))
        })
        .with_layer_fn(|router| {
            router.layer(axum::middleware::from_fn(|req, next: axum::middleware::Next| async move {
                let mut resp = next.run(req).await;
                resp.headers_mut().insert("x-plugin-b", HeaderValue::from_static("b"));
                resp
            }))
        })
        .build();

    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers().get("x-plugin-a").unwrap().to_str().unwrap(), "a");
    assert_eq!(resp.headers().get("x-plugin-b").unwrap().to_str().unwrap(), "b");
}

#[tokio::test]
async fn with_layer_fn_applied() {
    use axum::http::HeaderValue;

    let router = build_app()
        .with(Health)
        .with_layer_fn(|router| {
            router.layer(axum::middleware::from_fn(|req, next: axum::middleware::Next| async move {
                let mut resp = next.run(req).await;
                resp.headers_mut().insert("x-custom-layer", HeaderValue::from_static("applied"));
                resp
            }))
        })
        .build();

    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("x-custom-layer").unwrap().to_str().unwrap(),
        "applied"
    );
}

#[tokio::test]
async fn with_state_bypasses_bean_graph() {
    // with_state() skips the bean graph entirely — verify it builds a working router.
    let router = AppBuilder::new().with_state(()).with(Health).build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── E.4 Controller Registration ───────────────────────────────────────────

#[tokio::test]
async fn register_routes_adds_handler() {
    use axum::routing::get;

    let router = build_app()
        .register_routes(axum::Router::new().route("/test", get(|| async { "ok" })))
        .build();
    let (status, body) = send_get(router, "/test").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn multiple_route_registrations_merge() {
    use axum::routing::get;

    let router = build_app()
        .register_routes(axum::Router::new().route("/a", get(|| async { "alpha" })))
        .register_routes(axum::Router::new().route("/b", get(|| async { "beta" })))
        .build();

    let (status_a, body_a) = send_get(router.clone(), "/a").await;
    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(body_a, "alpha");

    let (status_b, body_b) = send_get(router, "/b").await;
    assert_eq!(status_b, StatusCode::OK);
    assert_eq!(body_b, "beta");
}

#[tokio::test]
async fn register_routes_with_state_access() {
    use axum::extract::State;
    use axum::routing::get;

    #[derive(Clone)]
    struct AppState {
        greeting: String,
    }

    let state = AppState {
        greeting: "hello".into(),
    };
    let router = AppBuilder::new()
        .with_state(state)
        .register_routes(axum::Router::new().route(
            "/greet",
            get(|State(s): State<AppState>| async move { s.greeting }),
        ))
        .build();

    let (status, body) = send_get(router, "/greet").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hello");
}

// ── E.5 NormalizePath edge cases ──────────────────────────────────────────

#[tokio::test]
async fn normalize_path_preserves_query_string() {
    let router = build_app().with(Health).with(NormalizePath).build();
    // /health/ with query string should redirect to /health?foo=bar
    let (status, body) = send_get(router, "/health/?foo=bar").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

#[tokio::test]
async fn normalize_path_root_slash_unaffected() {
    // GET / with no root route should return 404, not a redirect loop
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, _) = send_get(router, "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
