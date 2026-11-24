# example-sharded-bench

A minimal app for benchmarking sharded serving (`server.workers`,
`SO_REUSEPORT`, one runtime per worker) against the default single runtime.

Endpoints:

- `GET /plain`: a plain-text response
- `GET /json`: a small JSON body
- `GET /db`: a query against a file-backed SQLite database in a temp
  directory, seeded with 100 rows

## Running

```bash
cargo run -p example-sharded-bench --release
```

It listens on `http://127.0.0.1:3000` (`application.yaml`). The worker mode is
picked at runtime, without rebuilding:

```bash
R2E_SERVER_WORKERS=per-core cargo run -p example-sharded-bench --release   # one worker per core
R2E_SERVER_WORKERS=4        cargo run -p example-sharded-bench --release   # fixed count
cargo run -p example-sharded-bench --release                               # unset: single runtime
```

`tools/bench-sharded.sh` drives the comparison. Background and tuning:
`docs/features/19-sharded-serving.md`.
