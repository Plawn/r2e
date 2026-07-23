//! A controller-level (impl-level) `#[intercept(...)]` whose spec type cannot
//! be inferred must be a compile error — not a silent drop. The gate in
//! `ctrl_deco_set` is all-or-nothing, so without this error the valid sibling
//! interceptor below would silently stop running on every route too.

use r2e::prelude::*;

#[derive(Clone, Default)]
pub struct Logged;

impl SelfBuilt for Logged {}

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

fn make_interceptor() -> Logged {
    Logged
}

#[controller(path = "/c")]
pub struct Ctrl {}

#[routes]
#[intercept(Logged)]
#[intercept(make_interceptor())]
impl Ctrl {
    #[get("/a")]
    async fn a(&self) -> String {
        "a".into()
    }

    #[get("/b")]
    async fn b(&self) -> String {
        "b".into()
    }
}

fn main() {}
