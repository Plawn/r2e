//! Scaffolding for `llm/guards.md`.

use r2e::prelude::*;
use std::future::Future;

/// Not in the prelude: `use sqlx::PgPool;`.
pub use sqlx::PgPool;

/// The payload the guarded routes return.
#[derive(Clone, Default, serde::Serialize)]
pub struct Data {
    pub value: String,
}

/// The tuple-struct guard the `#[guard(RequireApiKey("x-api-key"))]` site
/// names — a `SelfBuilt` spec whose single field is the header to look for.
pub struct RequireApiKey(pub &'static str);

impl SelfBuilt for RequireApiKey {}

impl<I: Identity> Guard<I> for RequireApiKey {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Response>> + Send {
        let present = ctx.headers.contains_key(self.0);
        async move {
            if present {
                Ok(())
            } else {
                Err(GuardError::unauthorized("missing API key").into())
            }
        }
    }
}
