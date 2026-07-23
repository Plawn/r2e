//! `#[guard(MyGuard("key"))]` / `#[intercept(Tag("x"))]` — a single-segment
//! uppercase call is a tuple-struct constructor, so the spec type is the
//! path itself (DI backlog item 3). No `MyGuard = MyGuard("key")` escape
//! hatch needed.

use r2e::prelude::*;
use r2e::{GuardContext, Identity, InterceptorContext};
use std::future::Future;

pub struct RequireApiKey(pub &'static str);

impl SelfBuilt for RequireApiKey {}

impl<I: Identity> Guard<I> for RequireApiKey {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async move {
            if ctx.headers.contains_key(self.0) {
                Ok(())
            } else {
                Err(GuardError::forbidden("missing api key").into())
            }
        }
    }
}

pub struct Tagged(pub &'static str);

impl SelfBuilt for Tagged {}

impl<R: Send> Interceptor<R> for Tagged {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        async move { next().await }
    }
}

#[controller(path = "/t")]
pub struct TupleCtorController {}

#[routes]
impl TupleCtorController {
    #[get("/")]
    #[guard(RequireApiKey("x-api-key"))]
    #[intercept(Tagged("audit"))]
    async fn hello(&self) -> String {
        "ok".into()
    }
}

fn main() {
    let _ = async {
        AppBuilder::new()
            .build_state()
            .await
            .register_controller::<TupleCtorController>()
    };
}
