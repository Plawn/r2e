//! A "shared worker crate" built against `r2e` with `default-features = false`.
//!
//! The shape this canary protects: a team factors its background workers out
//! of the app crate into a library that several binaries depend on. That
//! library wants the service contract and nothing else — no security, no event
//! bus, no interceptor utilities — so every item it touches must be reachable
//! from a featureless `r2e`.
//!
//! What it exercises:
//!
//! - `#[derive(BackgroundService)]`, including `#[inject]` / `#[config]` fields
//!   and the `#[service(enabled = "…")]` gate;
//! - a **hand-written** `ServiceComponent` impl on a plain struct — the
//!   `#[producer(start)]` shape, where the worker has ordinary fields and the
//!   app builds it in a producer instead of writing a DI adapter struct;
//! - `r2e::rt::CancelToken` and `r2e::beans::BeanContext`, which the two above
//!   need in their signatures.
//!
//! Feature unification makes the guarantee observable only when this package is
//! built on its own — `cargo check -p r2e-featureless-tests`. A `--workspace`
//! build unifies `r2e`'s features across every member and would light up
//! `full` regardless.

use r2e::prelude::*;
use r2e::rt::CancelToken;
use r2e::type_list::{TCons, TNil};
use r2e::{BeanContext, ServiceComponent};

// ── 1. A worker with no R2E attributes at all ───────────────────────────────
//
// Plain fields, a plain constructor, an inherent `run`. This is the struct a
// shared crate wants to expose: it knows nothing about the bean graph, and the
// app that owns the graph builds it in a `#[producer(start)]`.

/// Stands in for the client/pool such a worker would take — the app provides
/// it as a bean.
#[derive(Clone, Default)]
pub struct Sink {
    pub name: &'static str,
}

#[derive(Clone)]
pub struct Reindexer {
    pub sink: Sink,
    pub batch_size: usize,
}

impl Reindexer {
    pub fn new(sink: Sink, batch_size: usize) -> Self {
        Self { sink, batch_size }
    }

    pub async fn run(&self, shutdown: CancelToken) {
        shutdown.cancelled().await;
    }
}

// The `#[producer(start)]` contract: the produced value IS a bean, so the
// service reads itself back out of the graph. Three lines, no adapter struct,
// and it lives in the shared crate next to the worker because nothing in it is
// app-specific.
impl ServiceComponent for Reindexer {
    type Deps = TCons<Reindexer, TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<Reindexer>()
    }

    async fn start(self, shutdown: CancelToken) {
        self.run(shutdown).await
    }
}

// ── 2. The derived form, with the `enabled` gate ────────────────────────────

/// Set by [`GatedWorker::run`], so the runtime test can assert the gate really
/// short-circuits `start`.
pub static GATED_WORKER_RAN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(BackgroundService)]
#[service(enabled = "enabled")]
pub struct GatedWorker {
    #[config("workers.gated.enabled")]
    pub enabled: bool,
    #[inject]
    pub sink: Sink,
}

impl GatedWorker {
    pub async fn run(&self, shutdown: CancelToken) {
        GATED_WORKER_RAN.store(true, std::sync::atomic::Ordering::SeqCst);
        shutdown.cancelled().await;
    }
}

/// The method form of the gate, on a struct with no config field.
#[derive(BackgroundService)]
#[service(enabled = "should_run")]
pub struct MethodGatedWorker {
    #[inject]
    pub sink: Sink,
}

impl MethodGatedWorker {
    fn should_run(&self) -> bool {
        self.sink.name != "off"
    }

    pub async fn run(&self, shutdown: CancelToken) {
        shutdown.cancelled().await;
    }
}
