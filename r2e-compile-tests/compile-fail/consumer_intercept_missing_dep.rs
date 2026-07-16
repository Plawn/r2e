//! An `#[intercept(...)]` spec on a controller `#[consumer]` method reads a bean
//! the app never provided — must be rejected at `register_controller()`. The
//! consumer interceptor's `Deps` are folded into `EndpointDeps` exactly like
//! route/scheduled decorator deps.

use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;
use std::future::Future;
use std::sync::Arc;

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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Evt {
    pub n: u32,
}

#[controller]
pub struct Consumers {
    #[inject]
    event_bus: LocalEventBus,
}

#[routes]
impl Consumers {
    #[consumer(bus = "event_bus")]
    #[intercept(Audit::spec())]
    async fn on_evt(&self, _e: Arc<Evt>) {}
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .provide(LocalEventBus::new())
            .build_state()
            .await
            .register_controller::<Consumers>()
    };
}
