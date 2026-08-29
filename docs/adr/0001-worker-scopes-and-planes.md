# ADR 0001 — Injection scopes, worker locality and the control/data plane split

- Status: accepted (2026-08-29, Tasker #990)
- Supersedes: nothing. Complements `docs/features/19-sharded-serving.md`
  (mechanics) and `docs/claude/beans-di.md` (bean graph).

## Context

`server.workers` runs HTTP on `N` `current_thread` runtimes (one OS thread,
one `SO_REUSEPORT` listener each). Everything else R2E manages — scheduler,
executor, event consumers, QUIC, lazy-bean first touch — runs on the caller's
multi-thread runtime. proxy-mesh (RM-4.0) proved the primitives are sound
(`AppBuilder::per_worker_service`, `WorkerContext::spawn_local`), and that the
in-process multi-worker model is *not* the performance ceiling (4 shards in one
process vs 4 single-shard processes: +0.6 %). What was missing was a vocabulary:
which scope a value lives in, which thread may build/use/drop it, and how to
*see* when a "shared-nothing" service is secretly sharing.

## Decision

### 1. Two planes, named once

| Plane | Runtime | Runs | Reaches the other plane through |
|---|---|---|---|
| **Control plane** | the main multi-thread runtime (`#[r2e::main]`) | boot, bean graph, `#[scheduled]`, `#[consumer]`, `PoolExecutor`, QUIC, shutdown, lazy-bean first touch, WebSocket sessions after upgrade | `Mailboxes::send_to(worker, ..)` / `ask` |
| **Data plane** | one `current_thread` runtime per worker, thread `r2e-worker-{i}` | HTTP accept + handlers for that listener, per-worker services and their `spawn_local` tasks | `rt::spawn_ctl` (fire-and-forget to the control plane), `Mailboxes` replies |

Without `server.workers` there is only a control plane and every scope below
collapses onto it. **The single-listener behaviour is unchanged by this ADR.**

### 2. Four scopes, three thread contracts

| Scope | Declared with | Instances | Built on | Used on | Dropped on | Type bounds |
|---|---|---|---|---|---|---|
| App singleton | `.provide` / `.register` / plugin `Provided` | 1 per app | control plane, `build_state()` | any thread | control plane, shutdown | `Clone + Send + Sync + 'static` |
| Request | `#[inject(identity)]`, `#[inject(request)]`, handler params | 1 per request | the worker (or control-plane thread) serving the request | that request only | end of request | `FromRequestPartsVia` |
| **Worker-local** | `AppBuilder::worker_local(factory)` → bean `WorkerLocal<T>` | **exactly 1 per worker** | worker `i`'s thread, inside its `LocalSet`, before it accepts | worker `i` only (`WorkerLocal::with`) | worker `i`'s thread, after HTTP drain, before the runtime is torn down | `T: 'static` — **`Send`/`Sync` not required** (`Rc`, `RefCell`, adopted sockets are fine) |
| Per-worker service | `AppBuilder::per_worker_service(factory)` | exactly 1 per worker | same as worker-local | same | same, `WorkerService::shutdown` in reverse start order | service `'static`, factory `Send + Sync` |

`WorkerLocal<T>` is the DI face of the per-worker service scope: the handle is
`Clone + Send + Sync` and injectable like any bean (`#[inject] hits:
WorkerLocal<Counter>`), but the **value** is only reachable through
`with(|t| ..)`, which resolves against the *calling thread's* slot. Off a
worker thread — on the control plane, in a `TestApp::boot` handler, from
another worker — it is a panic with the worker id in the message, never a
silent fall-through to a shared instance. That asymmetry (handle shared,
value pinned) is the whole point: you cannot confuse it with a singleton
because the singleton API (`Deref`, `.clone()` of the value) does not exist.

### 3. Identity is data, not a capability

`WorkerContext` (`!Send + !Sync`) is the *capability*: it can `spawn_local`
and adopt sockets, so it may never leave its thread. `WorkerInfo` (`Copy +
Send + Sync`) is the *identity*: `id`, `workers`, `role`, effective `cpu`. It
is an infallible request extractor / `#[inject(request)]` target and is
readable anywhere through `WorkerInfo::current()` (`None` on the control
plane). Anything that only needs to *label* work uses `WorkerInfo`; anything
that needs to *own* work uses `WorkerContext`.

### 4. Crossing the plane boundary is explicit and counted

Cross-worker and control→worker communication goes through
`Mailboxes<M>`: one bounded channel per worker, receiver attached from inside
the worker (`attach(&ctx)`), senders shared. Every send is attributed to the
target worker's `WorkerSlot` as a **local** crossing (sender is the target
worker) or a **remote** crossing (any other thread), with queue depth and
wait time. A "shared-nothing" service whose remote-crossing counter climbs on
the hot path is not shared-nothing — the metric is the test.

### 5. Lifecycle is aggregated, not hand-rolled

`WorkerSet` (provide it as a bean to observe it) tracks every worker's state
machine — `Unstarted → Starting → Ready → Serving → Draining → ServicesDown →
Parked → Exited | Failed(reason)` — with `wait_all_serving()`,
`snapshot()`, and a readiness indicator (`WorkerHealth` plugin) that is `Up`
only while every worker is `Serving`, so an LB deregisters the instance at the
first `Draining` transition. Startup remains all-or-nothing; the first error
names the worker and the service index.

### 6. Ingress affinity is a contract with a visible failure

`runtime::ingress` owns the reuseport socket helpers (`reuseport_tcp`,
`reuseport_udp`, `WorkerContext::adopt_udp` / `adopt_tcp_listener`). On a
platform without `SO_REUSEPORT` they return `AffinityError::Unsupported`
instead of a plain socket: **promised affinity that cannot be honoured is an
error, never a silent shared socket**. QUIC/UDP protocols build on
`reuseport_udp` + adoption (one endpoint per worker); R2E does not own the
protocol on top.

## Consequences

- Nothing changes for apps without `server.workers`. Registering a
  `worker_local`/`per_worker_service` without sharding stays a hard `run()`
  error (there is no worker to own the value).
- Handlers stay `Send` (axum): worker-local values are reached inside
  `with(..)` closures, not held across `.await`. `!Send` ergonomics beyond
  that are option C territory (`19-sharded-serving.md`, "Future directions").
- CPU pinning is still not applied; `WorkerInfo::cpu` is `None` and documented
  as the effective affinity slot.
- Tests prove the model with `runtime::harness::WorkerHarness`: real worker
  threads + runtimes + `LocalSet`s, no HTTP, `run_on(worker, ..)` to execute on
  a chosen worker, `WorkerSet` counters for instances/drops/crossings.
