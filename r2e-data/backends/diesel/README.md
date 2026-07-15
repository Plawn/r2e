# r2e-data-diesel

Cancellation-safe managed Diesel transactions for R2E, backed by an r2d2
connection pool. This crate intentionally contains no repository abstraction.

```toml
[dependencies]
r2e = { version = "0.1", features = ["diesel-postgres"] }
diesel = { version = "2", features = ["postgres", "r2d2"] }
```

Register `Pool<ConnectionManager<C>>` with `.provide(pool)`, then use
`#[managed] tx: &mut r2e::r2e_data_diesel::DieselTx<C>`. Call `tx.run(...)`
from async handlers so synchronous Diesel queries execute on Tokio's blocking
pool.

Responses below 400 commit; `4xx`/`5xx` responses roll back. On panic or
cancellation the open connection is discarded instead of returned to r2d2.

Features: `sqlite`, `postgres`, `mysql`. The MySQL feature requires a native
`libmysqlclient`/MariaDB client library, as required by Diesel itself.
