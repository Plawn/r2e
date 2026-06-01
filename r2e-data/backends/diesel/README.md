# r2e-data-diesel

[Diesel](https://diesel.rs/) backend for the R2E data layer.

## Status

**Skeleton implementation** — this crate provides the foundation for a Diesel-backed data layer but is not fully implemented yet. Consider using [`r2e-data-sqlx`](../r2e-data-sqlx) for production workloads.

## Feature flags

| Feature | Driver |
|---------|--------|
| `sqlite` | SQLite via `diesel/sqlite` |
| `postgres` | PostgreSQL via `diesel/postgres` |
| `mysql` | MySQL via `diesel/mysql` |

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["data-diesel", "sqlite"] }
```

## License

Apache-2.0
