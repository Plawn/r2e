# Sharded Serving (SO_REUSEPORT)

## TL;DR

Serve HTTP with N worker threads, each a `current_thread` Tokio runtime with its own `SO_REUSEPORT` listener bound to the same address — the kernel spreads connections across them, with no accept-path work-stealing (thread-per-core option A; Axum and the ecosystem stay unchanged). Set `server.workers` in config: absent = single listener (default, unchanged); a positive integer = that many workers (even `1`); `"per-core"` = `available_parallelism()`. Invalid values (`0`, negative, `> 1024`, other strings) hard-error at `run()` — never a silent fallback.


R2E can serve HTTP with **N worker threads**, each running its own
`current_thread` Tokio runtime with its own `SO_REUSEPORT` listener bound to the
same address. The kernel distributes incoming connections across the per-worker
listeners, so there is no work-stealing on the accept path. This is *option A*
of the thread-per-core (TPC) plan: it keeps Axum and the entire ecosystem
unchanged — each worker simply serves a clone of the same router.

## Enabling

Set `server.workers` in configuration:

```yaml
server:
  host: "0.0.0.0"
  port: 3000
  workers: 4          # 4 worker threads, each with its own SO_REUSEPORT listener
```

Accepted values:

| Value | Effect |
|---|---|
| *(absent)* | Single listener on the caller's runtime (**default, unchanged**). |
| positive integer `n >= 1` | Sharded serving with `n` workers (even `n = 1`). |
| `"per-core"` | `n = std::thread::available_parallelism()`. |
| `0`, negative, `> 1024`, other strings | **Hard error** at `run()` time (never a silent fallback). The `1024` cap (`sharded::MAX_WORKERS`) catches config typos before FD/thread exhaustion. |

When `server.workers` is absent the behavior is byte-for-byte identical to
before this feature existed: a single listener bound on the caller's runtime.

## How it works

1. `AppBuilder::prepare()` parses `server.workers` (alongside `server.tcp_nodelay`)
   and stores the parsed result on `PreparedApp`. Parse errors are carried and
   surfaced at `run()` time.
2. `PreparedApp::run()` chooses a serve strategy:
   - `None` → single listener (existing path, unchanged).
   - `Some(n)` → sharded path.
3. The bind address is resolved **once** on the main runtime via async DNS
   (`tokio::net::lookup_host`, all candidates kept), never blocking std DNS on
   an async thread. The first listener tries each candidate in order until one
   binds — the same multi-address fallback as `tokio::net::TcpListener::bind`
   (e.g. `localhost` → `::1` then `127.0.0.1`). The remaining workers bind that
   listener's concrete `local_addr()`; this also makes **port `0` work**: the
   kernel assigns the ephemeral port once and every worker shares it.
4. Each worker's `socket2` socket is created with `set_reuse_address(true)` +
   `set_reuse_port(true)`, bound, `listen(1024)`, converted to a
   `std::net::TcpListener`, and set non-blocking (mandatory for
   `tokio::net::TcpListener::from_std`). A thread named `r2e-worker-{i}` builds a
   `current_thread` runtime and serves its listener with graceful shutdown.
5. `TCP_NODELAY` (`server.tcp_nodelay`, default `true`) is re-applied per worker
   listener using the same `ListenerExt::tap_io` + `set_nodelay(true)` pattern as
   the single-listener path.
6. Shutdown stays on the main runtime: on signal, plugin shutdown hooks fire,
   then a shared `CancellationToken` is cancelled. Each worker's graceful
   shutdown future is a `child_token().cancelled()`. The main thread then joins
   the worker threads; worker panics are logged via `tracing::error!`, and a
   worker's serve `Err` is propagated as the overall run error.
7. **Workers park before dying.** A worker runtime owns the I/O driver of every
   socket that worker accepted — including sockets that were *upgraded* and
   whose task now runs on the control plane (a `#[ws(...)]` session). So a
   drained worker does not drop its runtime: it reports "HTTP drained, services
   down" and waits inside `block_on` (still driving that I/O driver) until the
   control plane has joined the tracked handles, then returns. The handshake is
   `runtime::sharded::WorkerPark`; the release token is held through a
   cancel-on-drop guard, so a panicking or dropped `run()` still releases the
   workers. Without it a WebSocket session that reacted slowly to shutdown lost
   its socket mid-grace-period.

The lifecycle (consumer registration, serve/startup hooks, QUIC spawn, shutdown
phase, grace period) is **shared** with the single-listener path — only the
"bind + serve" middle section differs (`PreparedApp::run_inner` + the internal
`ServeStrategy` enum).

## When to use it

`server.workers` is **opt-in for a reason**: the default multi-thread runtime
(work-stealing across one shared accept loop) is the recommended starting point.
Sharding trades work-stealing's automatic load-balancing for per-core locality —
a trade that only pays under specific conditions. **Measure before switching.**

**It helps when all of these hold:**

- **CPU-bound, high-RPS serving.** The bottleneck is per-request CPU
  (serialization, parsing, light compute), not waiting on a downstream. Sharding
  removes the cross-core synchronization and task-migration overhead that
  work-stealing pays to stay balanced.
- **Many cores.** The win scales with core count: eliminating scheduler
  contention and improving cache locality matters more at 16–64 cores than at 2–4.
- **Short, homogeneous requests.** Every request costs roughly the same and
  finishes quickly (the TechEmpower profile). No request monopolizes its core.
- **Little contended shared state on the hot path.** State is still shared via
  `Arc` across workers (see "control plane / data plane" below); sharding helps
  most when the hot path does not hammer a single `RwLock`/`DashMap` that would
  bounce cache lines across cores anyway.

**It does NOT help (and can regress) when:**

- **Traffic is low / bursty.** With idle capacity, work-stealing's rebalancing is
  pure upside and sharding's pinning is pure downside. No measurable gain.
- **Workloads are dominated by downstream IO** (database, cache, upstream HTTP).
  The CPU saved on scheduling is noise next to the IO wait; the connection pool or
  the downstream is the real ceiling.
- **Shared-state contention dominates.** If a hot lock is the bottleneck, sharding
  the accept path does nothing — N workers still serialize on the same lock. Shard
  the *bean* instead (see "Future directions").
- **Few cores.** Below ~4 cores there is little scheduler contention to remove.
- **Long-tail / heterogeneous requests.** A connection is **pinned** to its
  accepting core for its lifetime. A slow or CPU-heavy request (a large stream, an
  un-`await`-ed CPU loop) blocks every other connection pinned to that core, with
  no rebalancing to rescue them. This is the classic thread-per-core footgun and
  the reason tokio/axum default to work-stealing.

**Rule of thumb:** start on the default multi-thread runtime. Reach for
`server.workers: per-core` only when a benchmark of *your* workload on your target
hardware shows the default runtime is scheduler-bound — and re-measure after
switching, because the answer is workload- and platform-specific (see the
[benchmark](#benchmark) below: on macOS the default runtime is generally faster,
with high run-to-run variance).

## Platform support

`SO_REUSEPORT` (via `socket2::Socket::set_reuse_port`) is only available on
unix targets, excluding solaris/illumos/cygwin. The sharded module is gated to:

```
#[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))))]
```

which mirrors socket2's own cfg for `set_reuse_port`. On unsupported platforms,
setting `server.workers` returns:

> `server.workers (SO_REUSEPORT sharding) is not supported on this platform`

## Limitations (v1)

- **Hot-reload (`dev-reload`) + sharding is unsupported.** When both are active,
  sharding is ignored (with a `tracing::warn!`) and the single cached-listener
  path is used.
- **`run_with_listener` ignores sharding.** The caller owns the (single)
  listener; if `server.workers` is set, a `tracing::warn!` is logged and serving
  proceeds single-listener.
- **R2E's own QUIC/HTTP3 endpoint is not sharded.** In sharded mode the QUIC
  endpoint (if configured) stays on the control plane exactly as today;
  sharding affects TCP only. Applications that want a *shard-local* QUIC (or
  any other UDP) endpoint build it themselves on top of
  [per-worker services](#per-worker-services) — R2E does not depend on Quinn.
- **A worker dying mid-run does not stop the app.** If one worker exits early
  (serve error or panic), the remaining workers keep serving — capacity is
  degraded by 1/N and nothing restarts the dead worker. The failure is logged
  immediately, but a worker serve `Err` only propagates as the overall run
  error after shutdown, when all workers are joined. (The single-listener path,
  by contrast, tears the whole server down on a serve error.)

## Per-worker services

Sharded serving gives you N `current_thread` runtimes, but by default they run
HTTP and nothing else — everything non-HTTP is routed to the control plane.
**Per-worker services** are the opt-in for a workload that must live *inside* a
worker: one QUIC endpoint per shard with its connection state, a per-shard UDP
socket, a shard-local cache or mailbox. The canonical use case is a Quinn
endpoint bound with `SO_REUSEPORT` on every worker, with `Rc`-based connection
state that never crosses a thread — the thread-per-core model that makes QUIC
scale.

```rust
use r2e::prelude::*;                    // WorkerContext, WorkerService
use r2e::runtime::worker::BoxError;

struct ShardEcho { count: Rc<RefCell<u64>>, loop_task: JobHandle<()> }

impl WorkerService for ShardEcho {
    fn shutdown(self: Box<Self>) -> LocalBoxFuture<'static, ()> {
        Box::pin(async move { let _ = self.loop_task.await; })
    }
}

AppBuilder::new()
    .load_config::<()>()
    .build_state().await
    .register_controller::<PingController>()
    .per_worker_service(move |worker: WorkerContext| async move {
        // One SO_REUSEPORT UDP socket per shard, adopted into THIS worker's runtime.
        let std_sock = bind_reuseport_udp(port)?;                // socket2, see example
        let sock = r2e::rt::UdpSocket::from_std(std_sock)?;
        let count = Rc::new(RefCell::new(0u64));                 // !Send is fine here
        let shutdown = worker.shutdown();
        let loop_task = worker.spawn_local(echo_loop(sock, count.clone(), shutdown));
        Ok::<_, BoxError>(ShardEcho { count, loop_task })
    })
```

Runnable version: `examples/example-worker-udp` (`cargo run -p example-worker-udp`,
then `echo hi | nc -u 127.0.0.1 4433` → `shard=<id> n=<count> hi`). Replace the
UDP echo loop with `quinn::Endpoint::new(config, None, std_sock, Arc::new(quinn::TokioRuntime))`
and you have shard-local QUIC without r2e knowing about Quinn.

### API

- `AppBuilder::per_worker_service(factory)` — any phase, repeatable; factories
  run in registration order. `factory: Fn(WorkerContext) -> impl Future<Output =
  Result<S, BoxError>>` must be `Send + Sync` (it is shared by all workers —
  capture `Arc`s / config); the **future and `S` need not be `Send`**.
- `WorkerContext` (`!Send + !Sync`): `id()` (stable `0..workers()`, also the
  thread-name suffix `r2e-worker-{id}`), `workers()`, `cpu()` (always `None` —
  R2E does not pin threads; reserved for when it does), `thread_id()`,
  `shutdown() -> CancelToken`, `spawn_local(fut) -> JobHandle` (a `!Send`-capable
  task on this worker only; panics if called off-thread, which the type makes
  impossible by accident).
- `WorkerService` (`'static`): one method, `shutdown(self: Box<Self>) ->
  LocalBoxFuture<'static, ()>`, defaulting to a no-op. `()` implements it —
  return `Ok(())` from a factory that has nothing to tear down.
- `r2e::rt::{spawn_local, LocalSet, UdpSocket}` are exposed for this purpose;
  `rt::spawn_local` is only valid inside a worker's `LocalSet` — prefer
  `WorkerContext::spawn_local`.

### Ownership and threading guarantees

1. **Exactly once per worker**, on the worker's own OS thread, inside its
   `current_thread` runtime and `LocalSet`, *after* the control-plane handle is
   registered and *before* the worker accepts its first connection.
2. Everything the factory produces stays on that thread for life: the service,
   `spawn_local` tasks, and any `Rc`/`RefCell` they share. Nothing can migrate
   to the control plane or another worker.
3. **All-or-nothing startup.** No worker serves until *every* worker has started
   *every* service. If a factory returns `Err` or panics, the failing worker
   shuts down what it already started (reverse order), every other worker is
   cancelled and does the same, and `run()` returns
   `worker {i}: per-worker service #{k} failed to start: {e}` (or
   `worker {i} exited before reporting startup (panicked?)`).
4. **Shutdown ordering per worker:** `worker.shutdown()` is cancelled together
   with the HTTP listener → in-flight HTTP drains → `WorkerService::shutdown`
   awaited for each service in reverse start order → the worker parks until the
   control plane's tracked-handle join is over → `LocalSet` and runtime
   dropped (which cancels any local task still running — await your tasks in
   `shutdown` if they must finish). A service is never dropped while its
   `shutdown` future is pending.
5. **Requires sharding.** `per_worker_service()` with `server.workers` unset,
   with `run_with_listener`, or under `dev-reload`, is a hard error at `run()`
   (`per_worker_service() is registered but server.workers is not set …`).
   Running the factory on the multi-thread control plane would silently break
   the `!Send` promise, so there is no fallback.
6. The control plane is untouched: scheduler, executor, consumers, and R2E's
   own QUIC endpoint keep their placement from the table above.

Platform note: on Linux the kernel hashes incoming UDP datagrams across the
`SO_REUSEPORT` group by 4-tuple, so shards share the load. macOS delivers all
datagrams of a reused UDP port to a single socket — the example runs there, but
only one shard sees traffic.

Tests: `r2e-core/tests/runtime/worker_services.rs` (1/2/4 workers: invocation
count and identity, `!Send` state + local tasks, reverse-order shutdown,
startup failure unwinding, panic, `StopHandle`).

## Which runtime executes what (control plane / data plane)

Sharded mode splits work across two kinds of runtime:

| Work | Runtime |
|---|---|
| HTTP request handling | **HTTP workers** (one `current_thread` runtime per worker — the *data plane*) |
| Scheduler tasks (`#[scheduled]`) | **Control plane** — one tracked driver task (`ServeContext::track`, a min-heap of next-fire times drives all schedules); tick bodies are submitted to the `PoolExecutor` (`spawn_ctl`, also control plane) |
| `ServiceComponent`s / `spawn_service` | Control plane |
| Event-bus consumers (`#[consumer]`), per-emit handler dispatch | Control plane |
| QUIC / HTTP3 endpoint (R2E's `server.quic.*`) | Control plane |
| Per-worker services (`per_worker_service`) + their `spawn_local` tasks | **HTTP workers** — inside each worker's `LocalSet`, `!Send` allowed |
| Executor pool jobs (`PoolExecutor::submit`/`try_submit`) | Control plane |
| Lazy-bean resolution (`#[bean(lazy)]` first-touch) | Control plane |

Each worker thread registers the control-plane handle (`r2e_core::rt::set_control_plane`)
before entering its runtime. Background work initiated from within a request
handler is routed back to the control plane via `r2e_core::rt::spawn_ctl`, so the
worker runtimes serve HTTP and nothing else. When `server.workers` is absent,
`spawn_ctl` is byte-for-byte equivalent to `spawn` — the default path is
untouched.

The `rt` module itself now lives in the **`r2e-rt`** crate at the bottom of the
workspace graph; `r2e_core::rt` is a re-export, so every path above (and
`r2e::rt::…`) resolves exactly as before. See
`plans/runtime-http-dependency-containment.md`.

Implication for `#[scheduled]` / `#[consumer]` code: the control plane is a
multi-thread runtime, so such code must be `Send` (it already is — nothing
changes for users). Lazy beans first touched from a request handler are now
resolved on the control plane (see below); no hidden fallback runtime is spun
up.

### Lazy beans

A lazy bean first touched from within a worker is resolved on the control-plane
runtime: the worker spawns the factory onto the control plane (via the
registered handle) and blocks on a channel for the result. It does **not** use
`block_in_place` (which panics on current-thread runtimes) and does **not**
require the `lazy-fallback-runtime` feature.

**This stalls the worker.** While the factory runs on the control plane, the
worker's entire `current_thread` runtime is blocked waiting for the result —
every other in-flight connection on that worker stops being served until the
factory completes. The path exists so a worker-side first-touch is *correct*,
not so it is cheap: resolve lazy beans eagerly during state construction (which
runs on the main runtime before serving) if they can be touched from request
handlers. If the factory panics, the original panic payload is re-raised on the
worker thread.

Known limitation: the circular-lazy-dependency detector does not see across
threads, so a factory that circularly re-touches the bean being resolved on a
worker deadlocks instead of panicking with a cycle trace. This already held with
the previous `lazy-fallback-runtime` behavior.

## Benchmark

### Methodology

A minimal benchmark app (`examples/example-sharded-bench`) exposes three GET
endpoints that isolate the cost we care about:

- `/plain` → plaintext `"Hello, World!"` (pure HTTP serving, no serialization)
- `/json` → a small serialized JSON object, a few fields (serialization-bound)
- `/db` → one sqlite `SELECT … WHERE id = ?` via sqlx, returning a small JSON row
  (a representative IO-touching endpoint; file-backed sqlite, 100 seeded rows)

The same release binary serves both modes; the serve mode is switched **at
runtime, without rebuilding**, via the `R2E_SERVER_WORKERS` environment variable
(config overlay → `server.workers`):

- unset → default single multi-thread runtime
- `R2E_SERVER_WORKERS=per-core` → SO_REUSEPORT sharding, one `current_thread`
  worker per core

Topology note: in sharded mode the process keeps the (mostly idle, parked) main
multi-thread runtime *in addition to* the N worker threads — roughly twice the
OS threads of the default mode on the same core count. That is the real shape of
a sharded deployment, so the comparison is deployment-realistic rather than
thread-count-equalized.

The load generator is [`oha`](https://github.com/hatoo/oha). Identical parameters
across both modes, with a short warmup discarded before each measured run:

```bash
# warmup (discarded), then measured run, per endpoint, per mode:
oha --no-tui --output-format json -z 2s  -c 64 http://127.0.0.1:3000/<endpoint>   # warmup
oha --no-tui --output-format json -z 10s -c 64 http://127.0.0.1:3000/<endpoint>   # measured
```

Everything above is automated by **`tools/bench-sharded.sh`** (build → run both
modes → collect RPS + p50/p99 → print the table). Re-run it with
`DURATION=20s CONNS=128 tools/bench-sharded.sh` to change the load profile.

### Results

> **These macOS numbers are INDICATIVE ONLY — do not treat them as the verdict
> for this feature.** macOS's `SO_REUSEPORT` does **not** implement Linux's
> documented load-balancing semantics for distributing TCP accepts across the
> per-worker listeners. On Darwin, `SO_REUSEPORT` lets multiple sockets bind the
> same port, but how the kernel spreads incoming connections across them is
> undocumented and observably noisier than Linux's even distribution.
> **Linux is the target platform for this feature.**
> The table below is structured so Linux rows can be appended later; regenerate it
> on Linux with `tools/bench-sharded.sh` (committed for exactly this purpose) and
> fill in the `Linux / x86_64` block.

Machine: Apple Silicon (arm64), macOS (Darwin 24.6.0), 10 logical cores.
Tools: `oha 1.14.0`, sqlx 0.8 (sqlite), tokio 1.52. Params: `-z 10s -c 64`, 2s warmup.

**macOS / arm64 — 10 cores (indicative only; SO_REUSEPORT lacks Linux accept balancing)**

| Endpoint | default RPS | default p50 (ms) | default p99 (ms) | per-core RPS | per-core p50 (ms) | per-core p99 (ms) |
|---|---|---|---|---|---|---|
| `/plain` | 220022 | 0.268 | 0.755 | 191421 | 0.279 | 1.101 |
| `/json`  | 218412 | 0.270 | 0.732 | 153625 | 0.297 | 1.708 |
| `/db`    | 102883 | 0.576 | 1.514 | 61477  | 0.934 | 2.777 |

**Linux / x86_64 — _N_ cores (TODO: regenerate on Linux with `tools/bench-sharded.sh`)**

| Endpoint | default RPS | default p50 (ms) | default p99 (ms) | per-core RPS | per-core p50 (ms) | per-core p99 (ms) |
|---|---|---|---|---|---|---|
| `/plain` | — | — | — | — | — | — |
| `/json`  | — | — | — | — | — | — |
| `/db`    | — | — | — | — | — | — |

**Reading the macOS results (reported as measured, not massaged):** the table is
one representative run out of four. The only **stable** signal across runs is
`/db`, where the default runtime wins by ~1.6× every time. On `/plain` and
`/json` the default runtime is usually ~10–30% ahead, but the ranking **flipped
on one run** (per-core ahead on both, with better p99) — run-to-run variance on
this platform is of the same order as the gap itself. Plausible contributors:
macOS's `SO_REUSEPORT` accept-distribution semantics are undocumented and differ
from Linux's explicit load-balancing, and a laptop is a thermally noisy bench
host. None of this is evidence about the feature either way; it is evidence that
the benchmark must be run on Linux (idle, fixed-frequency hardware if possible)
to be meaningful. On Linux, the design rationale and the literature (nginx,
actix/ntex worker models) predict per-core ahead on the CPU-bound `/plain` and
`/json` endpoints at this core count.

## Future directions

`server.workers` is *option A* of the thread-per-core (TPC) plan — the cheap,
ecosystem-preserving step. The full analysis of the alternatives lives in
`docs/research/thread-per-core.md`; this section is the durable summary of the two
options that remain on the table, with their trade-offs.

### Option D — hybrid io_uring data-plane listeners (additive, targeted zero-copy)

Keep tokio/axum serving everything it serves today (REST, auth, config, events,
DB-backed endpoints) and add
dedicated **thread-per-core io_uring listeners** for a handful of *hot* endpoints —
a `RawService`-style trait mounted on dedicated cores, fed from the rest of the app
via channels or lock-free state. This is **additive**: it does not touch the
existing architecture, every current feature keeps working, and you opt specific
paths into it.

- **Best for targeted zero-copy**, not blanket speedups. True network zero-copy
  (`splice`/`sendfile`, `MSG_ZEROCOPY`) requires *owning the socket*, which hyper
  never exposes — hence a data plane *outside* axum. It pays on proxying large
  payloads, streaming, and file downloads; it does **nothing** for a 2 KB JSON
  response where serialization dominates anyway.
- **Two distinct io_uring paths, do not conflate them:**
  - **fs ops** → native tokio io_uring behind `tokio_unstable`. As of tokio
    1.47/1.48 (2025), `fs::write` / `OpenOptions::open` use io_uring when enabled,
    and the tokio team's direction is to swap the file-API backend transparently.
    This eliminates syscalls and the `spawn_blocking` detour but data still
    transits userspace — a file throughput/latency win, **not** zero-copy. The
    plan is to enable it behind an r2e feature flag once it leaves unstable
    (automatic win for multipart-to-disk and future file IO, zero architecture
    change). **Do not take the `tokio-uring` crate dependency** — it is in
    semi-maintenance; native tokio is the fs-ops path.
  - **a dedicated zero-copy network data plane** → **monoio is preferred over
    `tokio-uring`** (active, more mature io_uring usage; tokio-uring's
    tokio-interop edge is irrelevant for an isolated, channel-fed data plane).
    With TLS this also needs kTLS, else userspace crypto reintroduces the copy.
- **Trade-off:** additive and surgical, but it is a *second* runtime with its own
  service model and channel plumbing — worth it only when a concrete zero-copy need
  appears and is measured, not as a default.

### Option C — pluggable HTTP engine (the ntex path; standalone project only)

R2E's user-facing surface is macro-generated: `#[routes]` emits axum handlers
today, and *could* emit handlers for another (thread-per-core, `!Send`) engine
behind a feature flag, keeping the controller/guard/DI model. Tempting, but the
replacement list is brutal and the verdict is **only viable as a dedicated
standalone project**:

- An own `Service` trait without mandatory `Send` (tower is unusable in `!Send`),
  own extractors (`FromRequestParts` is an axum trait), guards/interceptors as
  `LocalBoxFuture`, and an HTTP/1.1 (+h2?) codec on the target runtime.
- A **per-core state model**: `ContextConstruct::from_context` called once *per
  worker*, `#[inject]` accepting `!Send` types — ntex's factory pattern.
- An **ecosystem split**: sqlx, rdkafka, lapin, quinn are all tokio-bound. A
  monoio backend cannot reuse `r2e-data-sqlx`, the event backends, or current QUIC
  without cross-runtime channel bridges (and the latency they imply).
- **Trade-off:** months of work — essentially building ntex inside R2E — for a win
  that, for most handlers (SQL, JSON, network calls), TechEmpower-style benchmarks
  flatter far more than real production. Option A + `Bytes` everywhere + sendfile
  for static files already gets close to the ceiling reachable without rewriting
  the IO stack. Not worth doing inside R2E.

A natural intermediate, if benchmarks ever prove **shared-state contention** (not
accept scheduling) is the bottleneck under sharding, is a per-core bean scope
(`#[inject(per_worker)]`: `ContextConstruct::from_context` run once per worker so
each core gets its own instance). Even then, axum still requires `Send` handlers, so per-worker
types must be `Send` — you gain non-contention, not `!Send` ergonomics (that is
option C territory). Build none of this until a benchmark demands it.
