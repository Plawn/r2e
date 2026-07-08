use r2e::prelude::*;
use r2e::r2e_utils::Logged;
use std::future::Future;

#[derive(Clone)]
pub struct AppState;

pub struct AuditLog;

impl SelfBuilt for AuditLog {}

impl<R: Send> Interceptor<R> for AuditLog {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

#[controller(path = "/api")]
pub struct InterceptedController;

#[routes]
#[intercept(Logged::info())]
impl InterceptedController {
    #[get("/data")]
    #[intercept(AuditLog)]
    async fn get_data(&self) -> &'static str {
        "data"
    }
}

fn main() {}
