//! W10 phase 3: a controller core exercising all transverse features at once —
//! `#[post_construct]`, an intercepted `#[consumer]`, and a `#[scheduled]`
//! method — must compile.

use r2e::prelude::*;
use r2e::r2e_events::LocalEventBus;
use std::future::Future;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState;

#[derive(Clone, Default)]
pub struct Sink;

#[derive(DecoratorBean)]
pub struct Audit {
    #[inject]
    sink: Sink,
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
pub struct MixedController {
    #[inject]
    event_bus: LocalEventBus,
    #[inject]
    sink: Sink,
}

#[routes]
impl MixedController {
    #[post_construct]
    async fn init(&self) {
        let _ = &self.sink;
    }

    #[scheduled(every = 60)]
    async fn tick(&self) {}

    #[consumer(bus = "event_bus")]
    #[intercept(Audit::spec())]
    async fn on_evt(&self, _e: Arc<Evt>) {}
}

fn main() {}
