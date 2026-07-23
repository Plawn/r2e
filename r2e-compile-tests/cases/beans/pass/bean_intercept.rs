//! `#[intercept(...)]` on `#[bean]` `#[scheduled]`/`#[consumer]` methods —
//! W10 phase 2. `#[bean]` on the struct injects the hidden decorator slot;
//! `#[bean]` on the impl splits intercepted methods into a wrapper + inner and
//! folds the spec `Deps` into the bean's registration deps.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use r2e::prelude::*;
use r2e::r2e_utils::Logged;

// A bean-reading interceptor spec (non-TNil Deps).
#[derive(Clone, Default)]
pub struct Audit;

#[derive(DecoratorBean)]
pub struct AuditSpec {
    #[inject]
    audit: Audit,
    tag: &'static str,
}

impl<R: Send> Interceptor<R> for AuditSpec {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        let _ = (&self.audit, self.tag);
        async move { next().await }
    }
}

#[bean]
#[derive(Clone)]
pub struct CleanupService {
    ticks: Arc<AtomicUsize>,
}

#[bean]
#[intercept(Logged::info())]
impl CleanupService {
    pub fn new(ticks: Arc<AtomicUsize>) -> Self {
        Self { ticks }
    }

    // Self-built interceptor (Logged) + a bean-reading one (AuditSpec).
    #[scheduled(every = 10)]
    #[intercept(AuditSpec::spec("purge"))]
    async fn purge(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }

    // Sync source promoted to `async fn`.
    #[scheduled(every = 3600, name = "sync_purge")]
    fn sync_purge(&self) {
        self.ticks.fetch_add(1, Ordering::SeqCst);
    }
}

// The generated impls are usable as their bounds.
fn assert_source<T: ScheduledSource>() {}
fn assert_slot<T: r2e::r2e_core::HasDecoSlot>() {}
fn assert_fill<T: r2e::r2e_core::BeanDecoFill>() {}

fn main() {
    assert_source::<CleanupService>();
    assert_slot::<CleanupService>();
    assert_fill::<CleanupService>();
}
