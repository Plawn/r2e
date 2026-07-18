# Managed resources

`#[managed]` gives a route parameter an acquire/finalize lifecycle protected by
an RAII abort guard.

```rust
pub trait ManagedResource<S>: Sized + Send {
    type Error: Into<Response>;

    fn acquire(
        context: ManagedContext<'_, S>,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send;

    fn finalize(
        &mut self,
        outcome: &ManagedOutcome,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn abort(&mut self);
}
```

`ManagedContext` exposes the bean state plus controller and handler names.
`ManagedOutcome` contains the built response status; statuses below 400 are
successes. `abort` must be synchronous, infallible, non-blocking, and safe to
call when async finalization cannot run.

## Custom resource

```rust
use r2e::prelude::*;

struct AuditContext {
    entries: Vec<String>,
}

impl<S: Send + Sync> ManagedResource<S> for AuditContext {
    type Error = ManagedErr<HttpError>;

    async fn acquire(
        _context: ManagedContext<'_, S>,
    ) -> Result<Self, Self::Error> {
        Ok(Self { entries: Vec::new() })
    }

    async fn finalize(
        &mut self,
        outcome: &ManagedOutcome,
    ) -> Result<(), Self::Error> {
        tracing::info!(
            success = outcome.is_success(),
            status = %outcome.status,
            entries = ?self.entries,
            "request audit completed",
        );
        Ok(())
    }

    fn abort(&mut self) {
        // Only synchronous best-effort cleanup is allowed here.
        self.entries.clear();
    }
}

#[post("/work")]
async fn work(
    &self,
    #[managed] audit: &mut AuditContext,
) -> Result<StatusCode, HttpError> {
    audit.entries.push("work started".into());
    Ok(StatusCode::NO_CONTENT)
}
```

With several resources, acquisition follows parameter order and finalization
uses reverse order. All finalizers run even when one fails. A resource remains
armed until its finalizer succeeds, so a failed or cancelled finalizer also
falls back to `abort`.

See [Managed database transactions](../data-access/transactions.md) for the
ready-to-use SQLx and Diesel implementations.
