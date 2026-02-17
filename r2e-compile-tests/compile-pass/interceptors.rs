use r2e::prelude::*;
use r2e::r2e_utils::Logged;
use std::future::Future;

#[derive(Clone)]
pub struct AppState;

pub struct AuditLog;

impl<R: Send, S: Send + Sync> Interceptor<R, S> for AuditLog {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext<'_, S>,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

#[derive(Controller)]
#[controller(path = "/api", state = AppState)]
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
