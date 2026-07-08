//! An `#[intercept(...)]` spec on a `#[scheduled]` method reads a bean the
//! app never provided — must be rejected at `register_controller()`.
//! Scheduled interceptors are built from the bean context inside
//! `scheduled_tasks_boxed`, and their `Deps` are folded into
//! `ControllerDeps` exactly like route decorator deps.

use r2e::prelude::*;
use std::future::Future;

/// The bean the interceptor needs — deliberately never provided.
#[derive(Clone)]
pub struct AuditSink;

#[derive(DecoratorBean)]
pub struct Audit {
    #[inject]
    sink: AuditSink,
}

impl<R: Send> Interceptor<R> for Audit {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let _ = &self.sink;
        async move { next().await }
    }
}

#[controller]
pub struct Jobs {}

#[routes]
impl Jobs {
    #[scheduled(every = 60)]
    #[intercept(Audit::spec())]
    async fn tick(&self) {}
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_controller::<Jobs>()
    };
}
