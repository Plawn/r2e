# Pagination and managed transactions

The former generic `Entity`, `Repository`, `QueryBuilder`, and `DataError`
layer has been removed. It was not consumed by either backend and duplicated
the query APIs applications already use directly.

`Pageable` and `Page<T>` now live in `r2e-core` and are exported by
`r2e::prelude::*` without a feature flag.

Database integrations are intentionally limited to R2E-specific managed
transaction lifecycles:

- `sqlx-sqlite`, `sqlx-postgres`, `sqlx-mysql`;
- `diesel-sqlite`, `diesel-postgres`, `diesel-mysql`.

See the book's [managed database transaction guide](../book/src/data-access/transactions.md)
for setup and route examples.
