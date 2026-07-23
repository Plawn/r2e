# Handoff — Thread-per-core & performance work (r2e + proxy-mesh)

> Started 2026-06-11. Pruned 2026-07-23: everything verified as shipped was removed.
> Read this + `docs/research/thread-per-core.md` (design reference + open backlog) and
> `METHODOLOGY-subagent-implementation.md` (how the waves were run).

## Status

**The r2e side of the plan is COMPLETE and verified in the tree** — tasks 534 (`r2e_core::rt`
facade / `JobHandle`), 535 (executor public API on `JobHandle`), 536 (SO_REUSEPORT sharded
serving, `server.workers`), 537 (control-plane / data-plane split, `spawn_ctl`), 538 (docs +
benchmark), 544 (`server.tcp_nodelay`) are all implemented. See `r2e-core/src/rt.rs`,
`r2e-core/src/sharded.rs`, `r2e-core/src/builder/prepared.rs`, and
`docs/features/19-sharded-serving.md`.

**The proxy-mesh wire block is COMPLETE** — 539 (TCP_NODELAY), 541 (HTTP body off MessagePack,
protocol v2), 542 (agent streams both directions), 540 (master streams both directions) landed
as local commits `ec23aa7`, `1b2cd68`, `45cda55`, `3bf2175` in the proxy-mesh repo.

## What is still open

1. **Linux benchmark run (r2e)** — the macOS numbers in
   `docs/features/19-sharded-serving.md` § Benchmark are indicative only (macOS
   `SO_REUSEPORT` has no accept load-balancing). The Linux block of the table is still
   empty; regenerate it on idle Linux hardware with `tools/bench-sharded.sh` against
   `examples/example-sharded-bench` and fill it in.
2. **proxy-mesh push gate (user decision)** — 541/542/540 are LOCAL ONLY. A push to origin
   master triggers GitLab CI auto-deploy. **Deploy masters before agents** (PROTOCOL_VERSION=2
   handshake: master Kicks mismatched agents and sends `Welcome{protocol}` first).
3. **proxy-mesh 543 — tunnel relay via `copy_bidirectional`**, LAST and **measure-first**
   (premise re-verified: the mpsc hop is still there). The tunnel `try_send→Full→spawn`
   reorder hazard found during 541 belongs to this task. Bench before/after or don't merge.
4. **r2e perf backlog** — never filed as tasks; see `thread-per-core.md` §6 (stall detector,
   `#[offload]`/`#[blocking]`, `threads_per_worker`, `#[inject(per_worker)]`, dispatch accept)
   and §8/§9 (SIMD JSON, validated-token cache, moka cache backend, hot-path allocations,
   minimal-telemetry mode, native-tokio io_uring watch item, continuous bench harness).

## Deferred / known limitations carried forward

**r2e (sharded serving, v1):**
- Worker death = degraded capacity, no restart.
- dev-reload + sharding unsupported (warns, falls back to a single listener).
- A worker-side lazy-bean first touch stalls that worker's whole runtime — eager resolution
  recommended; documented in `19-sharded-serving.md`.
- Cross-thread circular lazy deps deadlock instead of panicking with a trace.

**proxy-mesh (bounded, deliberate):**
- Unbounded pending maps keyed by misbehaving-master ids; no agent-side independent body cap;
  no per-request agent timeout (agent task can wait forever on a body that never arrives if
  the master dies mid-request — master side resolves via `fail_node`).
- `bytes_up` recorded as 0 for early-responding upstreams (metrics-only race); registry entry
  can linger after client disconnect until the next agent frame; the 504 arm leaves the
  spawned body task streaming to a dead id.
- Ops note: the OnceLock e2e harness leaves orphan master/agent processes inheriting the
  stdout pipe — redirect e2e output to a file and pkill orphans after.
- `cargo test -p proxy-e2e` needs `--features quic` (non-default there; without it only the
  WS subset runs).

## Open questions for the user (never answered)

- File the remaining perf axes (`thread-per-core.md` §6/§8/§9) as Tasker tasks?
