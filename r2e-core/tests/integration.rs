use http_body_util::BodyExt;
use r2e_core::builder::AppBuilder;
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::plugins::{Cors, DevReload, ErrorHandling, Health, NormalizePath};
use r2e_core::request_id::RequestIdPlugin;
use r2e_core::secure_headers::SecureHeaders;
use tower::ServiceExt;

async fn send_get(router: r2e_core::http::Router, path: &str) -> (StatusCode, String) {
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).to_string())
}

async fn send_get_with_header(
    router: r2e_core::http::Router,
    path: &str,
    header_name: &str,
    header_value: &str,
) -> r2e_core::http::Response {
    let req = Request::builder()
        .uri(path)
        .header(header_name, header_value)
        .body(Body::empty())
        .unwrap();
    router.oneshot(req).await.unwrap()
}

async fn send_request(
    router: r2e_core::http::Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> r2e_core::http::Response {
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

#[r2e_core::test]
async fn health_returns_200_ok() {
    let router = build_app().with(Health).build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── ErrorHandling plugin ────────────────────────────────────────────────

#[r2e_core::test]
async fn error_handling_catches_panic() {
    use r2e_core::http::routing::get;

    let app = AppBuilder::new()
        .with_state(())
        .register_routes(r2e_core::http::Router::new().route(
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

#[r2e_core::test]
async fn normalize_path_strips_trailing() {
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, body) = send_get(router, "/health/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── DevReload plugin ────────────────────────────────────────────────────

#[r2e_core::test]
async fn dev_reload_status() {
    let router = build_app().with(DevReload).build();
    let (status, body) = send_get(router, "/__r2e_dev/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "dev");
}

#[r2e_core::test]
async fn dev_reload_ping() {
    let router = build_app().with(DevReload).build();
    let (status, body) = send_get(router, "/__r2e_dev/ping").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(json["boot_time"].is_number());
    assert_eq!(json["status"], "ok");
}

// ── RequestIdPlugin ─────────────────────────────────────────────────────

#[r2e_core::test]
async fn request_id_generated() {
    let router = build_app().with(Health).with(RequestIdPlugin).build();
    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp.headers().get("x-request-id");
    assert!(req_id.is_some(), "response should have X-Request-Id header");
    let val = req_id.unwrap().to_str().unwrap();
    assert!(!val.is_empty());
}

#[r2e_core::test]
async fn request_id_propagated() {
    let router = build_app().with(Health).with(RequestIdPlugin).build();
    let resp = send_get_with_header(router, "/health", "x-request-id", "test-123").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let req_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(req_id, "test-123");
}

// ── SecureHeaders plugin ────────────────────────────────────────────────

#[r2e_core::test]
async fn secure_headers_in_response() {
    let router = build_app()
        .with(Health)
        .with(SecureHeaders::default())
        .build();
    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);

    let headers = resp.headers();
    assert_eq!(
        headers
            .get("x-content-type-options")
            .unwrap()
            .to_str()
            .unwrap(),
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

#[r2e_core::test]
async fn advanced_health_all_up() {
    let router = build_app()
        .with(Health::builder().check(AlwaysUp).build())
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
}

#[r2e_core::test]
async fn advanced_health_one_down() {
    let router = build_app()
        .with(Health::builder().check(AlwaysUp).check(AlwaysDown).build())
        .build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "DOWN");
}

#[r2e_core::test]
async fn advanced_health_liveness_always_ok() {
    let router = build_app()
        .with(Health::builder().check(AlwaysDown).build())
        .build();
    let (status, _body) = send_get(router, "/health/live").await;
    assert_eq!(status, StatusCode::OK);
}

#[r2e_core::test]
async fn advanced_health_readiness_filters() {
    // LivenessOnly check is down but doesn't affect readiness
    let router = build_app()
        .with(
            Health::builder()
                .check(AlwaysUp)
                .check(LivenessOnly)
                .build(),
        )
        .build();
    let (status, body) = send_get(router, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
}

// ── E.1 CORS Plugin ──────────────────────────────────────────────────────

#[r2e_core::test]
async fn cors_permissive_allows_origin() {
    let router = build_app().with(Health).with(Cors::permissive()).build();
    let resp = send_get_with_header(router, "/health", "origin", "http://example.com").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("access-control-allow-origin").is_some(),
        "response should have access-control-allow-origin header"
    );
}

#[r2e_core::test]
async fn cors_preflight_returns_200() {
    let router = build_app().with(Health).with(Cors::permissive()).build();
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

use r2e_core::beans::{AsyncBean, Bean, BeanContext, BeanRegistry, Producer, Registrable};
use r2e_core::type_list::{BeanAccess, TCons, TNil};
use std::any::TypeId;

#[derive(Clone, Debug)]
struct TestDep(i32);

#[r2e_core::test]
async fn build_state_with_provide() {
    let router = AppBuilder::new()
        .provide(TestDep(42))
        .build_state()
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
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> Self {
        TestService {
            label: "built".into(),
        }
    }
}

impl Registrable for TestService {
    type Provided = Self;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register::<Self>();
    }
}

#[r2e_core::test]
async fn build_state_with_bean() {
    let router = AppBuilder::new()
        .register::<TestService>()
        .build_state()
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
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn build(_ctx: &BeanContext) -> impl std::future::Future<Output = Self> + Send + '_ {
        async {
            AsyncService {
                label: "async-built".into(),
            }
        }
    }
}

impl Registrable for AsyncService {
    type Provided = Self;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register_async::<Self>();
    }
}

#[r2e_core::test]
async fn build_state_with_async_bean() {
    let router = AppBuilder::new()
        .register::<AsyncService>()
        .build_state()
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
    type Deps = TNil;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }
    fn produce(_ctx: &BeanContext) -> impl std::future::Future<Output = Self::Output> + Send + '_ {
        async { ProducedValue("produced".into()) }
    }
}

impl Registrable for TestProducer {
    type Provided = ProducedValue;
    type Deps = TNil;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register_producer::<Self>();
    }
}

#[r2e_core::test]
async fn build_state_with_producer() {
    let router = AppBuilder::new()
        .register::<TestProducer>()
        .build_state()
        .await
        .with(Health)
        .build();
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
}

#[r2e_core::test]
async fn build_state_with_config_injection() {
    use r2e_core::config::{ConfigValue, R2eConfig};

    let mut config = R2eConfig::empty();
    config.set("app.name", ConfigValue::String("test-app".into()));
    // Provide both R2eConfig (for config injection) and TestDep (for state)
    let router = AppBuilder::new()
        .provide(config)
        .provide(TestDep(99))
        .build_state()
        .await
        .with(Health)
        .build();
    let (status, _) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
}

// ── E.2b Conditional availability via `#[producer] -> Option<T>` ──────────
//
// Runtime-flag conditional bean presence is expressed through a producer whose
// `Output = Option<T>`: the `Option<T>` slot is ALWAYS in the provision list,
// and the producer decides `Some`/`None` at build time from a flag it reads out
// of the context (`ServiceEnabled` / `ProducedEnabled`). Consumers hard-depend
// on `Option<T>` — no runtime `try_get` / missing-dependency escape hatch. The
// flag itself is a plain provided bean, computed either from a literal boolean
// or from config via `config_flag`.

// The runtime flag the producers key off, injected as a first-class bean.
#[derive(Clone)]
struct ServiceEnabled(bool);

// Producer for `Option<TestService>` — always registers the slot, emits
// `Some`/`None` based on the injected `ServiceEnabled` flag.
struct MaybeServiceProducer;

impl Producer for MaybeServiceProducer {
    type Output = Option<TestService>;
    type Deps = TCons<ServiceEnabled, TNil>;
    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<ServiceEnabled>(), "ServiceEnabled")]
    }
    fn produce(ctx: &BeanContext) -> impl std::future::Future<Output = Self::Output> + Send + '_ {
        let enabled = ctx.get::<ServiceEnabled>().0;
        async move {
            enabled.then(|| TestService {
                label: "built".into(),
            })
        }
    }
}

impl Registrable for MaybeServiceProducer {
    type Provided = Option<TestService>;
    type Deps = TCons<ServiceEnabled, TNil>;
    fn register_into(registry: &mut BeanRegistry) {
        registry.register_producer::<Self>();
    }
}

#[r2e_core::test]
async fn producer_option_present_when_flag_true() {
    let prepared = AppBuilder::new()
        .provide(ServiceEnabled(true))
        .register::<MaybeServiceProducer>()
        .build_state()
        .await
        .with(Health)
        .prepare("127.0.0.1:0");
    assert!(
        prepared.state().get::<Option<TestService>>().is_some(),
        "flag=true → Option<TestService> present"
    );
}

#[r2e_core::test]
async fn producer_option_absent_when_flag_false() {
    let prepared = AppBuilder::new()
        .provide(ServiceEnabled(false))
        .register::<MaybeServiceProducer>()
        .build_state()
        .await
        .with(Health)
        .prepare("127.0.0.1:0");
    assert!(
        prepared.state().get::<Option<TestService>>().is_none(),
        "flag=false → Option<TestService> absent"
    );
}

#[r2e_core::test]
async fn async_producer_option_present() {
    // A producer performs its work in an async body — this replaces the old
    // "conditional async bean" path. The slot is `Option<AsyncService>`.
    #[derive(Clone)]
    struct MaybeAsyncProducer;
    impl Producer for MaybeAsyncProducer {
        type Output = Option<AsyncService>;
        type Deps = TCons<ServiceEnabled, TNil>;
        fn dependencies() -> Vec<(TypeId, &'static str)> {
            vec![(TypeId::of::<ServiceEnabled>(), "ServiceEnabled")]
        }
        fn produce(
            ctx: &BeanContext,
        ) -> impl std::future::Future<Output = Self::Output> + Send + '_ {
            let enabled = ctx.get::<ServiceEnabled>().0;
            async move {
                enabled.then(|| AsyncService {
                    label: "async-built".into(),
                })
            }
        }
    }
    impl Registrable for MaybeAsyncProducer {
        type Provided = Option<AsyncService>;
        type Deps = TCons<ServiceEnabled, TNil>;
        fn register_into(registry: &mut BeanRegistry) {
            registry.register_producer::<Self>();
        }
    }

    let prepared = AppBuilder::new()
        .provide(ServiceEnabled(true))
        .register::<MaybeAsyncProducer>()
        .build_state()
        .await
        .with(Health)
        .prepare("127.0.0.1:0");
    assert!(prepared.state().get::<Option<AsyncService>>().is_some());
}

#[r2e_core::test]
async fn producer_option_present_via_config_flag() {
    use r2e_core::config::{ConfigValue, R2eConfig};

    let mut config = R2eConfig::empty();
    config.set("features.test-service", ConfigValue::Bool(true));

    // Compute the flag from config via the public `config_flag` helper before
    // consuming the builder, then feed it into the producer.
    let builder = AppBuilder::new()
        .override_config(config)
        .load_config::<()>();
    let enabled = builder.config_flag("features.test-service");
    assert!(enabled);

    let prepared = builder
        .provide(ServiceEnabled(enabled))
        .register::<MaybeServiceProducer>()
        .build_state()
        .await
        .with(Health)
        .prepare("127.0.0.1:0");
    assert!(prepared.state().get::<Option<TestService>>().is_some());
}

#[r2e_core::test]
async fn producer_option_absent_via_config_flag() {
    use r2e_core::config::{ConfigValue, R2eConfig};

    let mut config = R2eConfig::empty();
    config.set("features.test-service", ConfigValue::Bool(false));

    let builder = AppBuilder::new()
        .override_config(config)
        .load_config::<()>();
    let enabled = builder.config_flag("features.test-service");
    assert!(!enabled);

    let prepared = builder
        .provide(ServiceEnabled(enabled))
        .register::<MaybeServiceProducer>()
        .build_state()
        .await
        .with(Health)
        .prepare("127.0.0.1:0");
    assert!(prepared.state().get::<Option<TestService>>().is_none());
}

#[r2e_core::test]
async fn config_flag_missing_key_defaults_to_false() {
    use r2e_core::config::R2eConfig;

    let builder = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>();
    // Missing key → `config_flag` yields false → producer emits `None`.
    let enabled = builder.config_flag("features.nonexistent");
    assert!(!enabled);

    let prepared = builder
        .provide(ServiceEnabled(enabled))
        .register::<MaybeServiceProducer>()
        .build_state()
        .await
        .with(Health)
        .prepare("127.0.0.1:0");
    assert!(prepared.state().get::<Option<TestService>>().is_none());
}

#[r2e_core::test]
async fn when_applies_transformation_conditionally() {
    // `when` runs a `Self -> Self` transformation only when the flag is true.
    let with_plugin = build_app().when(true, |b| b.with(Health)).build();
    let (status, _) = send_get(with_plugin, "/health").await;
    assert_eq!(status, StatusCode::OK);

    let without_plugin = build_app().when(false, |b| b.with(Health)).build();
    let (status, _) = send_get(without_plugin, "/health").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn profile_is_reflects_active_profile() {
    use r2e_core::config::{ConfigValue, R2eConfig};

    let mut config = R2eConfig::empty();
    config.set("r2e.profile", ConfigValue::String("prod".into()));

    let builder = AppBuilder::new()
        .override_config(config)
        .load_config::<()>();
    // `R2E_PROFILE` (if set in the environment) overrides the config key, so
    // pin the expectation to whatever the resolver actually chose.
    let active = builder.active_profile().to_string();
    assert!(builder.profile_is(&active));
    assert!(!builder.profile_is("definitely-not-the-active-profile"));
    if std::env::var("R2E_PROFILE").is_err() {
        assert_eq!(active, "prod");
    }
}

// ── E.3 Plugin Lifecycle ──────────────────────────────────────────────────

#[r2e_core::test]
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

#[r2e_core::test]
async fn plugin_ordering_layers_respected() {
    use r2e_core::http::HeaderValue;

    let router = build_app()
        .with(Health)
        .with_layer_fn(|router| {
            router.layer(r2e_core::http::middleware::from_fn(
                |req, next: r2e_core::http::middleware::Next| async move {
                    let mut resp = next.run(req).await;
                    resp.headers_mut()
                        .insert("x-plugin-a", HeaderValue::from_static("a"));
                    resp
                },
            ))
        })
        .with_layer_fn(|router| {
            router.layer(r2e_core::http::middleware::from_fn(
                |req, next: r2e_core::http::middleware::Next| async move {
                    let mut resp = next.run(req).await;
                    resp.headers_mut()
                        .insert("x-plugin-b", HeaderValue::from_static("b"));
                    resp
                },
            ))
        })
        .build();

    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("x-plugin-a").unwrap().to_str().unwrap(),
        "a"
    );
    assert_eq!(
        resp.headers().get("x-plugin-b").unwrap().to_str().unwrap(),
        "b"
    );
}

#[r2e_core::test]
async fn with_layer_fn_applied() {
    use r2e_core::http::HeaderValue;

    let router = build_app()
        .with(Health)
        .with_layer_fn(|router| {
            router.layer(r2e_core::http::middleware::from_fn(
                |req, next: r2e_core::http::middleware::Next| async move {
                    let mut resp = next.run(req).await;
                    resp.headers_mut()
                        .insert("x-custom-layer", HeaderValue::from_static("applied"));
                    resp
                },
            ))
        })
        .build();

    let resp = send_get_with_header(router, "/health", "accept", "*/*").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("x-custom-layer")
            .unwrap()
            .to_str()
            .unwrap(),
        "applied"
    );
}

#[r2e_core::test]
async fn with_state_bypasses_bean_graph() {
    // with_state() skips the bean graph entirely — verify it builds a working router.
    let router = AppBuilder::new().with_state(()).with(Health).build();
    let (status, body) = send_get(router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

// ── E.4 Controller Registration ───────────────────────────────────────────

#[r2e_core::test]
async fn register_routes_adds_handler() {
    use r2e_core::http::routing::get;

    let router = build_app()
        .register_routes(r2e_core::http::Router::new().route("/test", get(|| async { "ok" })))
        .build();
    let (status, body) = send_get(router, "/test").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[r2e_core::test]
async fn multiple_route_registrations_merge() {
    use r2e_core::http::routing::get;

    let router = build_app()
        .register_routes(r2e_core::http::Router::new().route("/a", get(|| async { "alpha" })))
        .register_routes(r2e_core::http::Router::new().route("/b", get(|| async { "beta" })))
        .build();

    let (status_a, body_a) = send_get(router.clone(), "/a").await;
    assert_eq!(status_a, StatusCode::OK);
    assert_eq!(body_a, "alpha");

    let (status_b, body_b) = send_get(router, "/b").await;
    assert_eq!(status_b, StatusCode::OK);
    assert_eq!(body_b, "beta");
}

#[r2e_core::test]
async fn register_routes_with_state_access() {
    use r2e_core::http::routing::get;
    use r2e_core::http::State;

    #[derive(Clone)]
    struct AppState {
        greeting: String,
    }

    let state = AppState {
        greeting: "hello".into(),
    };
    let router = AppBuilder::new()
        .with_state(state)
        .register_routes(r2e_core::http::Router::new().route(
            "/greet",
            get(|State(s): State<AppState>| async move { s.greeting }),
        ))
        .build();

    let (status, body) = send_get(router, "/greet").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "hello");
}

// ── E.5 NormalizePath edge cases ──────────────────────────────────────────

#[r2e_core::test]
async fn normalize_path_preserves_query_string() {
    let router = build_app().with(Health).with(NormalizePath).build();
    // /health/ with query string should redirect to /health?foo=bar
    let (status, body) = send_get(router, "/health/?foo=bar").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}

#[r2e_core::test]
async fn normalize_path_root_slash_unaffected() {
    // GET / with no root route should return 404, not a redirect loop
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, _) = send_get(router, "/").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[r2e_core::test]
async fn normalize_path_preserves_matched_path_for_outer_layers() {
    // The rewrite happens BEFORE routing, so a trailing-slash request is
    // routed exactly once and instrumentation layers (Prometheus, OTel)
    // added via `with_layer_fn` see the `MatchedPath` route template —
    // not the "unmatched" sentinel a fallback re-dispatch would leave.
    use r2e_core::http::extract::MatchedPath;
    use r2e_core::http::middleware::{from_fn, Next};
    use r2e_core::http::routing::get;

    let router = build_app()
        .register_routes(
            r2e_core::http::Router::new().route("/users/{id}", get(|| async { "user" })),
        )
        .with(NormalizePath)
        .with_layer_fn(|router| {
            router.layer(from_fn(|req: Request<Body>, next: Next| async move {
                let label = req
                    .extensions()
                    .get::<MatchedPath>()
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| "unmatched".to_string());
                let mut resp = next.run(req).await;
                resp.headers_mut()
                    .insert("x-matched-path", label.parse().unwrap());
                resp
            }))
        })
        .build();

    let resp = send_request(router, "GET", "/users/42/", &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["x-matched-path"], "/users/{id}");
}

#[r2e_core::test]
async fn normalize_path_collapses_leading_slashes() {
    // tower-http's trim_trailing_slash also collapses a leading run of
    // slashes (`//health` → `/health`) — documented plugin behavior.
    // Absolute-form URI keeps `//health` as the path (origin-form `//x`
    // would parse as an authority).
    let router = build_app().with(Health).with(NormalizePath).build();
    let (status, body) = send_get(router, "http://test//health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");
}
