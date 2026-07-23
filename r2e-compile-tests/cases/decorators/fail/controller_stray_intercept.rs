//! `#[intercept]` on a plain controller method (not a route / #[scheduled] /
//! #[consumer]) has no dispatch wrapper to run the chain — reject it rather
//! than silently ignore (parity with the bean-side check).

use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[derive(Clone, Default)]
pub struct Logged;

impl<R: Send> Interceptor<R> for Logged {
    fn around<F, Fut>(
        &self,
        _ctx: InterceptorContext,
        next: F,
    ) -> impl std::future::Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[intercept(Logged)]
    async fn helper(&self) {}
}

fn main() {}
