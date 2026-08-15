//! A `#[managed]` parameter type must declare its bean dependencies through
//! `ManagedDeps`. There is no blanket impl: forgetting it is a compile error,
//! not a resource that silently depends on nothing.

use r2e::prelude::*;
use r2e::{HttpError, ManagedContext, ManagedErr, ManagedOutcome, ManagedResource};

pub struct Audit;

impl<S: Send + Sync> ManagedResource<S> for Audit {
    type Error = ManagedErr<HttpError>;

    async fn acquire(_context: ManagedContext<'_, S>) -> Result<Self, Self::Error> {
        Ok(Self)
    }

    async fn finalize(&mut self, _outcome: &ManagedOutcome) -> Result<(), Self::Error> {
        Ok(())
    }

    fn abort(&mut self) {}
}

// Missing: `impl ManagedDeps for Audit { type Deps = TNil; }`

#[controller(path = "/audit")]
pub struct AuditController;

#[routes]
impl AuditController {
    #[get("/")]
    async fn record(&self, #[managed] _audit: &mut Audit) -> String {
        "ok".to_string()
    }
}

fn main() {}
