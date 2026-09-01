//! Scaffolding for `llm/scheduled-tasks.md`.

use r2e::prelude::*;
use r2e::BeanContext;

pub use std::sync::atomic::{AtomicUsize, Ordering};
pub use std::sync::Arc;

pub use sqlx::SqlitePool;

/// The `Executor` plugin is not in the prelude (`use r2e::r2e_executor::Executor;`).
pub use r2e::r2e_executor::Executor;

/// `decorate()` on a hand-built instance comes from `use r2e::Decorate;`.
pub use r2e::Decorate;

/// The collaborator the controller snippet injects.
#[derive(Clone)]
pub struct UserService;

#[bean]
impl UserService {
    pub fn new() -> Self {
        Self
    }

    pub async fn count(&self) -> usize {
        0
    }

    pub async fn sync(&self) {}
}

/// The controller the setup snippet registers — the doc's own block redefines
/// it with the full set of `#[scheduled]` variants.
#[controller]
pub struct ScheduledJobs {}

#[routes]
impl ScheduledJobs {
    #[scheduled(every = 30)]
    async fn count_users(&self) {}
}

/// State for the dynamic-task snippet (`ctx.get::<SyncService>()`).
#[derive(Clone)]
pub struct SyncService;

#[bean]
impl SyncService {
    pub fn new() -> Self {
        Self
    }

    pub async fn sync(&self) {}
}

/// The test double the `decorate` / `override_bean_decorated` snippet pins.
#[derive(Clone)]
pub struct Stub;

#[allow(non_upper_case_globals)]
pub const stub: Stub = Stub;

/// The intercepted bean of the `Decorate` snippet (the doc's earlier blocks
/// define their own `CleanupService`; this one backs the fragment that only
/// *uses* it).
#[bean]
#[derive(Clone)]
pub struct CleanupService {
    stub: Stub,
}

#[bean]
impl CleanupService {
    pub fn new(dep: Stub) -> Self {
        Self { stub: dep }
    }

    #[scheduled(every = "5m")]
    #[intercept(AuditTick::spec("purge"))]
    pub async fn purge(&self) {
        let _ = &self.stub;
    }
}

/// A user-defined interceptor spec (`#[intercept(AuditTick::spec("…"))]`).
pub struct AuditTick {
    label: &'static str,
}

impl AuditTick {
    pub fn spec(label: &'static str) -> Self {
        Self { label }
    }
}

pub struct AuditTickInterceptor {
    pub label: &'static str,
}

impl<R> Interceptor<R> for AuditTickInterceptor {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let label = self.label;
        async move {
            tracing::debug!(audit = label, "tick");
            next().await
        }
    }
}

impl DecoratorSpec for AuditTick {
    type Product = AuditTickInterceptor;
    type Deps = r2e::type_list::TNil;

    fn build(self, _ctx: &BeanContext) -> AuditTickInterceptor {
        AuditTickInterceptor { label: self.label }
    }
}
