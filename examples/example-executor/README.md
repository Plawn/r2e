# example-executor

The managed task pool (`r2e-executor`): fire-and-forget jobs from a handler and
a long-running background service, both drained on graceful shutdown.

What it shows:

- `#[async_exec]` on a controller method (`generate_pdf`): calling it submits
  the body to the `Executor` pool and returns a `JobHandle` at once (or a
  `RejectedError` when the pool is full)
- A `#[derive(BackgroundService)]` worker (`TickWorker`) started at boot and
  stopped on shutdown
- A shared `Arc<AtomicU64>` counter, provided from `App::setup`, and the pool
  metrics exposed by a route
- No YAML: the app boots on `override_config(R2eConfig::empty())`

## Running

```bash
cargo run -p example-executor
```

The server listens on `http://localhost:3000`.

```bash
curl -X POST localhost:3000/reports/123   # fire and forget: {"status":"queued"}
curl localhost:3000/reports/123           # awaits the job inline: {"status":"ready"}
curl localhost:3000/metrics               # running/queued/completed/rejected + ticks
```

See `docs/claude/executor.md` and the book's executor chapter for the API.
