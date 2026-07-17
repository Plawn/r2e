//! Controller-level (impl-level) `#[intercept]` instance sharing (W5).
//!
//! An impl-level interceptor on a `#[routes]` block is built **once per
//! controller** and shared across every route (and every `#[scheduled]` /
//! `#[consumer]` method), NOT once per route. This matters for stateful
//! interceptors: a counter kept inside the interceptor must advance across
//! calls to *different* routes of the same controller.
//!
//! Proof strategy: a manual `DecoratorSpec` whose `build` mints a **fresh**
//! per-instance sequence counter. On each call the interceptor records its own
//! next sequence number into a shared `Evidence` bean. If the interceptor is a
//! single shared instance, calls across two routes yield a strictly increasing
//! sequence `[1, 2, 3, ...]`. If it were rebuilt per route, each route would
//! own an independent counter and the sequence would reset (`[1, 2, 1]`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use r2e::beans::BeanContext;
use r2e::config::R2eConfig;
use r2e::prelude::*;
use r2e::{DecoratorSpec, Interceptor, InterceptorContext, TCons, TNil};
use r2e_test::TestApp;

// ─── Shared evidence bean (records each interceptor call's sequence number) ───

#[derive(Clone, Default)]
pub struct Evidence {
    seqs: Arc<Mutex<Vec<usize>>>,
}

impl Evidence {
    fn record(&self, n: usize) {
        self.seqs.lock().unwrap().push(n);
    }
    fn snapshot(&self) -> Vec<usize> {
        self.seqs.lock().unwrap().clone()
    }
}

// ─── A stateful interceptor with a fresh per-INSTANCE counter ───

/// The finished interceptor (product). `seq` is minted per instance in
/// `build`, so observing a single monotonically increasing sequence across
/// routes proves there is exactly one instance.
pub struct SeqCounter {
    seq: Arc<AtomicUsize>,
    evidence: Evidence,
}

impl<R: Send> Interceptor<R> for SeqCounter {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let ev = self.evidence.clone();
        async move {
            ev.record(n);
            next().await
        }
    }
}

/// The spec named by the attribute. `build` reads the shared `Evidence` bean
/// and mints a brand-new counter for THIS instance.
pub struct SeqCounterSpec;

impl DecoratorSpec for SeqCounterSpec {
    type Product = SeqCounter;
    type Deps = TCons<Evidence, TNil>;
    fn build(self, ctx: &BeanContext) -> SeqCounter {
        SeqCounter {
            seq: Arc::new(AtomicUsize::new(0)),
            evidence: ctx.get(),
        }
    }
}

// ─── Controller with two routes under one impl-level interceptor ───

#[derive(Clone)]
pub struct Svc;

#[controller(path = "/shared")]
pub struct SharedController {
    #[inject]
    _svc: Svc,
}

#[routes]
#[intercept(SeqCounterSpec)]
impl SharedController {
    #[get("/a")]
    async fn route_a(&self) -> Json<&'static str> {
        Json("a")
    }

    #[get("/b")]
    async fn route_b(&self) -> Json<&'static str> {
        Json("b")
    }
}

async fn setup() -> TestApp {
    TestApp::from_builder(
        AppBuilder::new()
            .override_config(R2eConfig::empty())
            .load_config::<()>()
            .provide(Svc)
            .provide(Evidence::default())
            .build_state()
            .await
            .register_controller::<SharedController>(),
    )
}

// ─── Test ───

#[r2e::test]
async fn controller_level_interceptor_is_shared_across_routes() {
    let app = setup().await;

    // Two hits on /a and one on /b — interleaved across the two routes.
    app.get("/shared/a").send().await.assert_ok();
    app.get("/shared/b").send().await.assert_ok();
    app.get("/shared/a").send().await.assert_ok();

    let evidence = app.bean::<Evidence>();
    let seqs = evidence.snapshot();

    // A single shared instance yields a strictly increasing sequence across
    // BOTH routes. Per-route instances would reset the counter (e.g. [1, 1, 2]).
    assert_eq!(
        seqs,
        vec![1, 2, 3],
        "controller-level interceptor must be one shared instance across routes"
    );
}
