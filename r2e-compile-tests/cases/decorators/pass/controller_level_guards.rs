//! Controller-level `#[guard]`/`#[pre_guard]`/`#[roles]` on the `#[routes]`
//! impl block (task #906): inherited by every route, cumulated with the
//! method-level decorators, `#[anonymous]` opting out of the post-auth ones.

use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use r2e::{Guard, GuardContext, Identity, PreAuthGuardContext};
use std::future::Future;

pub struct HeaderGuard(&'static str);

impl SelfBuilt for HeaderGuard {}

impl<I: Identity> Guard<I> for HeaderGuard {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let ok = ctx.headers.contains_key(self.0);
        async move {
            if ok {
                Ok(())
            } else {
                Err(GuardError::forbidden("missing header").into())
            }
        }
    }
}

pub struct AllowAllPre;

impl SelfBuilt for AllowAllPre {}

impl PreAuthGuard for AllowAllPre {
    fn check(
        &self,
        _ctx: &PreAuthGuardContext<'_>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        async { Ok(()) }
    }
}

/// Every route inherits the controller guard + pre-guard + roles; `list`
/// cumulates a method-level guard and pre-guard on top; `health` is
/// `#[anonymous]` (opts out of identity and the controller's post-auth
/// checks, keeps the pre-guard).
#[controller(path = "/inherited")]
pub struct InheritedController {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
#[guard(HeaderGuard("x-api-key"))]
#[pre_guard(AllowAllPre)]
#[roles("member")]
impl InheritedController {
    #[get("/")]
    #[guard(HeaderGuard("x-extra"))]
    #[pre_guard(AllowAllPre)]
    async fn list(&self) -> String {
        self.user.sub().to_string()
    }

    #[get("/detail")]
    #[roles("admin")]
    async fn detail(&self) -> &'static str {
        "detail"
    }

    #[get("/health")]
    #[anonymous]
    async fn health(&self) -> &'static str {
        "ok"
    }
}

fn main() {}
