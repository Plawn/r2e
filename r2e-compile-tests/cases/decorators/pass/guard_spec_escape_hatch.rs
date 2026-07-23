//! The explicit `SpecType = expr` form names the spec type when the
//! expression has no usable leading type path (free functions, variables).

use r2e::prelude::*;
use r2e::{GuardContext, Identity, InterceptorContext};
use std::future::Future;

pub struct AllowAll;

impl SelfBuilt for AllowAll {}

impl<I: Identity> Guard<I> for AllowAll {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

fn make_guard() -> AllowAll {
    AllowAll
}

pub struct PassThrough;

impl SelfBuilt for PassThrough {}

impl<R: Send> Interceptor<R> for PassThrough {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

fn make_interceptor() -> PassThrough {
    PassThrough
}

#[controller(path = "/g")]
pub struct GuardedController {}

#[routes]
impl GuardedController {
    #[get("/")]
    #[guard(AllowAll = make_guard())]
    #[intercept(PassThrough = make_interceptor())]
    async fn hello(&self) -> String {
        "ok".into()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_controller::<GuardedController>()
    };
}
