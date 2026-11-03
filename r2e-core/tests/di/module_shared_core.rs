//! One controller type mounted by two prefixed feature modules (#1215): the
//! builder keeps ONE core per type, so the lifecycle is wired once — one
//! `#[post_construct]` run, one `#[on_start]` run — while the routes are
//! mounted (and their decorators built) on every registration.

use http_body_util::BodyExt;
use r2e_core::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

/// Core-local: each constructed core clones the provided (empty) bean, so
/// it owns its own `OnceLock`; setting it twice means one core initialised
/// twice.
#[derive(Clone, Default)]
struct InitSlot(OnceLock<()>);

/// Shared counters (an `Arc` survives the per-core clone).
#[derive(Clone, Default)]
struct Runs {
    post_construct: Arc<AtomicUsize>,
    on_start: Arc<AtomicUsize>,
    intercepted: Arc<AtomicUsize>,
}

#[derive(DecoratorBean)]
struct Counting {
    #[inject]
    runs: Runs,
}

impl<R: Send> Interceptor<R> for Counting {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        self.runs.intercepted.fetch_add(1, Ordering::SeqCst);
        next()
    }
}

#[controller(path = "/state")]
struct Mounted {
    #[inject]
    slot: InitSlot,
    #[inject]
    runs: Runs,
}

#[routes]
impl Mounted {
    #[post_construct]
    async fn init(&self) {
        self.runs.post_construct.fetch_add(1, Ordering::SeqCst);
        self.slot.0.set(()).expect("core initialized twice");
    }

    #[on_start]
    async fn started(&self) {
        self.runs.on_start.fetch_add(1, Ordering::SeqCst);
    }

    #[get("/")]
    #[intercept(Counting::spec())]
    async fn state(&self) -> &'static str {
        if self.slot.0.get().is_some() {
            "ready"
        } else {
            "unset"
        }
    }
}

#[module(prefix = "/v1", controllers(Mounted), imports(InitSlot, Runs))]
struct V1;

#[module(prefix = "/v2", controllers(Mounted), imports(InitSlot, Runs))]
struct V2;

async fn get(router: &r2e_core::http::Routes, path: &str) -> (r2e_core::http::StatusCode, String) {
    let req = r2e_core::http::Request::builder()
        .uri(path)
        .body(r2e_core::http::Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[r2e_core::test]
async fn a_type_mounted_by_two_modules_has_one_core_and_one_lifecycle() {
    let runs = Runs::default();
    let router = r2e_core::AppBuilder::new()
        .provide(InitSlot::default())
        .provide(runs.clone())
        .register_module::<V1>()
        .register_module::<V2>()
        .build_state()
        .await
        .build_with_consumers()
        .await;

    assert_eq!(
        runs.post_construct.load(Ordering::SeqCst),
        1,
        "post_construct runs"
    );
    assert_eq!(runs.on_start.load(Ordering::SeqCst), 1, "on_start runs");

    for (n, path) in ["/v1/state", "/v2/state"].into_iter().enumerate() {
        let (status, body) = get(&router, path).await;
        assert_eq!(status, r2e_core::http::StatusCode::OK, "{path}");
        assert_eq!(body, "ready", "{path}");
        // One interceptor call per invocation, on either mount.
        assert_eq!(runs.intercepted.load(Ordering::SeqCst), n + 1, "{path}");
    }
}
