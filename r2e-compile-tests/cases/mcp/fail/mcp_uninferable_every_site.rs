//! On an `#[mcp_routes]` impl every non-inferable decorator spec is reported
//! — impl-level guard and interceptor, member-level guard and interceptor,
//! across members — exactly once each, not only the first bad site.

use r2e::prelude::*;
use r2e::{GuardContext, Identity, Interceptor, InterceptorContext};
use std::future::Future;

pub struct AllowAll;

impl SelfBuilt for AllowAll {}

impl<I: Identity> Guard<I> for AllowAll {
    fn check(
        &self,
        _ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Rejection>> + Send {
        async { Ok(()) }
    }
}

pub struct Through;

impl SelfBuilt for Through {}

impl<R: Send> Interceptor<R> for Through {
    fn around<F, Fut>(&self, _ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        next()
    }
}

fn make_guard() -> AllowAll {
    AllowAll
}

fn make_interceptor() -> Through {
    Through
}

#[controller]
pub struct EverySiteTools {}

#[mcp_routes]
#[guard(make_guard())]
#[intercept(make_interceptor())]
impl EverySiteTools {
    #[tool]
    #[guard(make_guard())]
    async fn first(&self) -> String {
        "ok".into()
    }

    #[tool]
    #[intercept(make_interceptor())]
    async fn second(&self) -> String {
        "ok".into()
    }
}

fn main() {}
