//! End-to-end coverage for `#[derive(DecoratorBean)]` and the
//! single-segment tuple-struct constructor spec inference
//! (DI backlog items 3 + 4).
//!
//! A real `#[controller]`/`#[routes]` controller exercises:
//!
//! - a derived **guard** with an `#[inject]` bean dep and a plain (config)
//!   field set at the site via the generated `spec(...)` constructor;
//! - a derived **interceptor** mixing `#[inject]`, `#[config("key")]`, and a
//!   plain field — config resolved from `R2eConfig` at wiring time;
//! - a `SelfBuilt` tuple-struct guard used as `#[guard(RequireHeader("x"))]`
//!   (single-segment call: the path itself is the spec type);
//! - guard order / short-circuit and the site-set build path through
//!   `build_decorator` (expression type = hidden companion spec, named type
//!   = product).

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use http_body_util::BodyExt;
use r2e_core::config::{ConfigValue, R2eConfig};
use r2e_core::http::response::Response;
use r2e_core::http::{Body, Request, StatusCode};
use r2e_core::prelude::*;
use r2e_core::{AppBuilder, GuardContext, Identity};
use tower::ServiceExt;

// ── Beans ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct HitCounter {
    hits: Arc<AtomicUsize>,
}

#[derive(Clone)]
pub struct AuditSink(Arc<Mutex<Vec<String>>>);

// ── Derived guard: #[inject] bean + plain config field ─────────────────────

#[derive(DecoratorBean)]
pub struct QuotaGuard {
    #[inject]
    counter: HitCounter,
    max: usize,
}

impl<I: Identity> Guard<I> for QuotaGuard {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            if self.counter.hits.fetch_add(1, Ordering::SeqCst) >= self.max {
                Err(GuardError::new(StatusCode::TOO_MANY_REQUESTS, "quota exhausted").into())
            } else {
                Ok(())
            }
        }
    }
}

// ── Derived interceptor: #[inject] + #[config] + section + plain field ─────

#[derive(ConfigProperties)]
pub struct AuditSection {
    suffix: String,
}

#[derive(DecoratorBean)]
pub struct Audit {
    #[inject]
    sink: AuditSink,
    #[config("audit.channel")]
    channel: String,
    #[config_section(prefix = "audit")]
    section: AuditSection,
    tag: &'static str,
}

impl<R: Send> Interceptor<R> for Audit {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let method_name = ctx.method_name;
        async move {
            self.sink.0.lock().unwrap().push(format!(
                "{}:{}:{}:enter {}",
                self.channel, self.section.suffix, self.tag, method_name
            ));
            let out = next().await;
            self.sink.0.lock().unwrap().push(format!(
                "{}:{}:{}:exit {}",
                self.channel, self.section.suffix, self.tag, method_name
            ));
            out
        }
    }
}

// ── SelfBuilt tuple-struct guard (single-segment ctor at the site) ─────────

pub struct RequireHeader(&'static str);

impl SelfBuilt for RequireHeader {}

impl<I: Identity> Guard<I> for RequireHeader {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            if ctx.headers.contains_key(self.0) {
                Ok(())
            } else {
                Err(GuardError::forbidden(format!("missing header {}", self.0)).into())
            }
        }
    }
}

// ── Escape hatch on a derived type: `Name = <prebuilt product>` ─────────────
//
// The identity `DecoratorSpec` impl on the product makes this compile; its
// `Deps` carrier still requires the beans (fail-closed over-requirement),
// even though this prebuilt instance uses its own sink.

static ESCAPE_SINK: std::sync::LazyLock<AuditSink> =
    std::sync::LazyLock::new(|| AuditSink(Arc::new(Mutex::new(Vec::new()))));

// ── Controller ──────────────────────────────────────────────────────────────

#[controller(path = "/d")]
pub struct DecoBeanController {}

#[routes]
impl DecoBeanController {
    #[get("/")]
    #[guard(RequireHeader("x-key"))]
    #[guard(QuotaGuard::spec(2))]
    #[intercept(Audit::spec("t1"))]
    async fn hello(&self) -> String {
        "ok".into()
    }

    #[get("/esc")]
    #[intercept(Audit = Audit {
        sink: ESCAPE_SINK.clone(),
        channel: "esc".into(),
        section: AuditSection { suffix: "s2".into() },
        tag: "t2",
    })]
    async fn escape(&self) -> String {
        "esc".into()
    }
}

// ── Test ────────────────────────────────────────────────────────────────────

#[r2e_core::test]
async fn derive_decorator_bean_end_to_end() {
    let counter = HitCounter {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let sink = AuditSink(Arc::new(Mutex::new(Vec::new())));

    let mut config = R2eConfig::empty();
    config.set("audit.channel", ConfigValue::String("audit".into()));
    config.set("audit.suffix", ConfigValue::String("s1".into()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .provide(counter.clone())
        .provide(sink.clone())
        .build_state()
        .await;

    let router = app.register_controller::<DecoBeanController>().build();

    let send = |with_header: bool| {
        let router = router.clone();
        async move {
            let mut req = Request::builder().uri("/d");
            if with_header {
                req = req.header("x-key", "1");
            }
            let resp = router
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = resp.status();
            let body = resp.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8(body.to_vec()).unwrap())
        }
    };

    // Header guard first (declaration order): short-circuits without
    // touching the quota or the interceptor.
    let (status, _) = send(false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(counter.hits.load(Ordering::SeqCst), 0);
    assert!(sink.0.lock().unwrap().is_empty());

    // Two requests within the quota set at the site (`spec(2)`).
    for _ in 0..2 {
        let (status, body) = send(true).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    // Quota exhausted → 429; the interceptor never ran for it.
    let (status, _) = send(true).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    // The interceptor's channel came from `#[config]`, its suffix from the
    // `#[config_section]`, its tag from the site's `spec("t1")` — all
    // resolved once at wiring time.
    assert_eq!(
        *sink.0.lock().unwrap(),
        vec![
            "audit:s1:t1:enter hello",
            "audit:s1:t1:exit hello",
            "audit:s1:t1:enter hello",
            "audit:s1:t1:exit hello",
        ]
    );

    // Escape hatch: the prebuilt product (identity DecoratorSpec impl) was
    // wired as-is — its own sink, no graph resolution for its fields.
    let router2 = router.clone();
    let resp = router2
        .oneshot(
            Request::builder()
                .uri("/d/esc")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        *ESCAPE_SINK.0.lock().unwrap(),
        vec!["esc:s2:t2:enter escape", "esc:s2:t2:exit escape"]
    );
}
