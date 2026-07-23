//! A bean whose `#[intercept]` spec reads a bean the app never provided must
//! be rejected — the spec's `Deps` is folded into the bean's registration
//! deps, so `build_state()` fails the `AllSatisfied` check like any missing
//! bean dependency.

use r2e::prelude::*;

/// The bean the interceptor needs — deliberately never provided.
#[derive(Clone)]
pub struct QuotaRegistry;

#[derive(DecoratorBean)]
pub struct QuotaAudit {
    #[inject]
    registry: QuotaRegistry,
    tag: &'static str,
}

impl<R: Send> Interceptor<R> for QuotaAudit {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        let _ = (&self.registry, self.tag);
        async move { next().await }
    }
}

#[bean]
#[derive(Clone)]
pub struct CleanupService {}

#[bean]
impl CleanupService {
    pub fn new() -> Self {
        Self {}
    }

    #[scheduled(every = 10)]
    #[intercept(QuotaAudit::spec("purge"))]
    async fn purge(&self) {}
}

fn main() {
    let _ = async {
        AppBuilder::new().register::<CleanupService>().build_state().await
    };
}
