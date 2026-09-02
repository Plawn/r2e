---
topic: runtime-facade
features: core
tokens: ~3000
requires: background-work, app-builder
---

## The runtime facade — `r2e::rt`

### TL;DR

- Reach for `r2e::rt::…` (= `r2e_core::rt`) instead of naming `tokio`: a `clippy.toml` `disallowed-methods` rule fails the build on the spawn family, and a raw `tokio::spawn` lands on the wrong runtime under sharded serving.
- `rt::spawn` is the data plane, `rt::spawn_ctl` the control plane (non-HTTP work), `rt::spawn_blocking` sync work; awaiting a `rt::JobHandle<T>` yields `Result<T, rt::JoinError>`.
- Serve-time work goes through `ServeContext::track`, never a bare `rt::spawn` — see llm/app-builder.md.
- Timers and IO come from the facade too: `rt::sleep`, `rt::timeout` (`rt::Elapsed`), `rt::interval`, `rt::sleep_until`, `rt::bind_tcp`, `rt::shutdown_signal`, and `rt::in_runtime()` as a non-panicking probe.
- Cancellation is `rt::CancelToken` (`cancelled().await` is cancellation-safe, plus `child_token()` and `drop_guard()`) — an app never needs `tokio-util` in its own `Cargo.toml`.
- `rt::sync::*`, `rt::{select!, pin!, join!}`, `rt::JoinSet`, `rt::stream`, `rt::io`, `rt::{TcpListener, TcpStream, UdpSocket}` are re-exports with unchanged identity.
- `#[r2e::main]` / `#[r2e::test]` / `#[r2e::test_suite]` build their runtime through `rt::RuntimeBuilder`, so a generated project needs no `tokio` entry; `start_paused = true` needs `r2e/test-util`.
- Shard-local `!Send` state: `AppBuilder::per_worker_service(factory)` runs the factory once inside every worker runtime and keeps the returned `WorkerService` alive; it requires `server.workers` and errors under `run_with_listener` or `dev-reload` rather than falling back.
- Inside a worker use `WorkerContext` (`id`, `workers`, `shutdown`, `spawn_local`, `adopt_udp`) — it is `!Send + !Sync`; `WorkerInfo` is the `Copy` identity readable anywhere, including as a handler parameter.
- One `T` per worker is `WorkerLocal<T>` (`with`/`try_with`), aggregated lifecycle is `WorkerSet`, cross-worker messaging is `Mailboxes<M>`, and per-shard sockets come from `r2e::runtime::ingress` (`reuseport_tcp`/`reuseport_udp`, never a silent single-socket fallback).

`r2e-rt` is the crate R2E puts between the app and `tokio`. Reach for
`r2e::rt::…` (= `r2e_core::rt`) instead of naming `tokio` directly: task
placement has to go through it for the control-plane / data-plane split to hold
under sharded serving (`server.workers`) — a raw `tokio::spawn` lands on
whatever runtime is current, which on a sharded worker thread is the wrong one.
A `clippy.toml` `disallowed-methods` rule fails the build on the spawn family.

```rust
use std::time::Duration;

use r2e::rt;

# async fn __doc(fut: impl std::future::Future<Output = ()>, deadline: rt::Instant)
#     -> Result<(), Box<dyn std::error::Error>> {
let h: rt::JobHandle<u32> = rt::spawn(async { 1 });   // data plane
rt::spawn_ctl(async { /* non-HTTP work */ });          // control plane
rt::spawn_blocking(|| heavy_sync());
h.await?;                                              // Result<T, rt::JoinError>

rt::sleep(Duration::from_millis(50)).await;
rt::timeout(Duration::from_secs(1), fut).await?;       // Err(rt::Elapsed)
let mut tick = rt::interval(Duration::from_secs(30));  // rt::Interval
rt::sleep_until(deadline).await;                       // deadline: rt::Instant
rt::yield_now().await;
let listener = rt::bind_tcp("0.0.0.0:3000").await?;
rt::shutdown_signal().await;                           // SIGINT / SIGTERM

if rt::in_runtime() { /* non-panicking probe, e.g. from a Drop impl */ }
# Ok(()) }
```

Cancellation is `rt::CancelToken` (a wrapper — an app never needs `tokio-util`
in its own `Cargo.toml`): `cancel()`, `is_cancelled()`, `cancelled().await`
(cancellation-safe, usable as a `select!` branch), `child_token()`, and
`drop_guard() -> rt::CancelDropGuard` for cancellation that survives a panic or
a dropped future. `From` converts both ways with
`tokio_util::sync::CancellationToken` if you need the raw one.

Also re-exported, identity unchanged, so the `tokio::` name disappears at zero
cost: `rt::sync::{mpsc, oneshot, broadcast, watch, Mutex, RwLock, Notify,
Semaphore, OnceCell, …}`, `rt::{select!, pin!, join!}`, `rt::JoinSet`,
`rt::stream` (`tokio-stream`), `rt::{TcpListener, TcpStream, UdpSocket}`, `rt::io`
(`AsyncRead`/`AsyncWrite` + the `…Ext` traits), `rt::{LocalSet, spawn_local}`
(only meaningful inside a per-worker service — prefer
`WorkerContext::spawn_local`), and
`rt::{RuntimeBuilder, Runtime, RuntimeHandle, RuntimeId, block_on,
block_in_place}` for owning a runtime yourself (`Runtime::id()` /
`RuntimeHandle::id()` give the `RuntimeId`, comparable across threads — use it
to assert two pieces of work share one reactor; ids are unique only among
*live* runtimes and may be reused once a runtime is dropped, so only compare
against a runtime you know is still alive. `Runtime::shutdown_timeout(dur)` /
`shutdown_background()` shut down a runtime you cannot drop, e.g. one parked in
a `static`).

`#[r2e::main]` / `#[r2e::test]` / `#[r2e::test_suite]` build their runtime
through `rt::RuntimeBuilder`, so a generated project needs **no** `tokio` entry
in its own `Cargo.toml`. `start_paused = true` needs the paused clock, which is
behind a non-default feature: enable `r2e/test-util` (a crate depending on
`r2e-test` already gets it in its dev graph).

Serve-time work still goes through `ServeContext::track`, never a bare
`rt::spawn` — see llm/app-builder.md.

### Per-worker services (shard-local `!Send` state)

Under sharded serving each worker is a `current_thread` runtime that serves HTTP
only. `AppBuilder::per_worker_service(factory)` (any phase, repeatable) runs
`factory(WorkerContext)` **exactly once inside every worker's runtime** — after
the worker exists, before it accepts traffic — and keeps the returned
`WorkerService` alive until shutdown. The factory must be `Send + Sync` (shared
by all workers); the future it returns and the service **may be `!Send`**
(`Rc`, `RefCell`, sockets adopted into the worker runtime). This is how an app
gets one Quinn/QUIC or UDP endpoint per shard with thread-local connection
state, without r2e depending on Quinn.

```rust
use std::cell::RefCell;
use std::rc::Rc;

use r2e::prelude::*;                         // WorkerContext, WorkerService
use r2e::rt::JobHandle;
use r2e::runtime::worker::{BoxError, LocalBoxFuture};

struct Shard { hits: Rc<RefCell<u64>>, task: JobHandle<()> }
impl WorkerService for Shard {
    fn shutdown(self: Box<Self>) -> LocalBoxFuture<'static, ()> {
        Box::pin(async move { let _ = self.task.await; })   // default impl is a no-op
    }
}

# fn __doc(builder: AppBuilder, port: u16) -> impl Sized {
builder.per_worker_service(move |worker: WorkerContext| async move {
    let sock = r2e::rt::UdpSocket::from_std(bind_reuseport_udp(port)?)?; // socket2
    let hits = Rc::new(RefCell::new(0u64));
    let stop = worker.shutdown();                       // CancelToken, fires at shutdown
    let task = worker.spawn_local(run(sock, hits.clone(), stop));
    tracing::info!(id = worker.id(), of = worker.workers(), cpu = ?worker.cpu());
    Ok::<_, BoxError>(Shard { hits, task })
})
# }
# fn main() {}
```

`WorkerContext` is `!Send + !Sync`: `id()` (stable `0..workers()`, thread name
`r2e-worker-{id}`), `workers()`, `cpu()` (always `None` today — no affinity),
`thread_id()`, `shutdown()`, `spawn_local(fut)`. `()` implements
`WorkerService` for factories with nothing to tear down. Guarantees: startup is
all-or-nothing (no worker serves until every worker started every service; a
failing or panicking factory unwinds already-started services everywhere and
`run()` errors with `worker {i}: per-worker service #{k} failed to start: …`);
shutdown per worker is HTTP drain → `WorkerService::shutdown` in reverse start
order → `LocalSet` dropped. Requires `server.workers` — with it unset, under
`run_with_listener`, or under `dev-reload`, `run()` returns an error rather
than falling back to the multi-thread control plane. Example:
`examples/example-worker-udp`. Reference: `docs/features/19-sharded-serving.md`.

### Worker scopes, lifecycle, mailboxes, ingress, metrics, test harness

Layered on `per_worker_service` (ADR `docs/adr/0001-worker-scopes-and-planes.md`;
all in `r2e::prelude` unless noted):

- **`WorkerInfo`** (`Copy`): stable identity readable anywhere — `id()`,
  `workers()`, `cpu()` (`Option<usize>`, effective affinity), `role()`
  (`WorkerRole::DataPlane | ControlPlane`), `is_data_plane()`;
  `WorkerInfo::current()` / `current_or_control_plane()`, `WorkerContext::info()`,
  and as a **handler parameter** (infallible; "control-plane" when not sharded).
  `Display` = `worker 3/8` / `control-plane`.
- **`WorkerLocal<T: 'static>`** — exactly one `T` per worker, `T` may be `!Send`.
  Handle is `Clone + Send + Sync` (provide as a bean, `#[inject]` it).
  `WorkerLocal::new(|w: WorkerContext| async { Ok::<_, BoxError>(t) })`;
  `.worker_local(factory)` on the builder = `.provide(local)` +
  `.per_worker_service(install)`. Read with `with(|t| ..)` (panics off-worker /
  before install / after drop) or `try_with(..) -> Option<R>`; `install(ctx) ->
  WorkerLocalGuard<T>` (a `WorkerService`; drops `T` on its own thread after HTTP
  drain). Counters: `instances()`, `built()`, `dropped()`, `is_installed()`.
- **`WorkerSet`** — aggregated lifecycle; `.provide(WorkerSet::new())` and the
  builder picks it up like `StopHandle`. `WorkerState`: `Unstarted → Starting →
  Ready → Serving → Draining → ServicesDown → Parked → Exited`, or `Failed`.
  `snapshot() -> Vec<WorkerSnapshot>` (`Serialize`), `states()`, `all_serving()`,
  `any_failed()`, `first_error() -> Option<(usize, String)>`, `slot(i)`,
  `wait_all_serving()`, `wait_all_exited()`, `wait_until(pred)`.
  **`WorkerHealth::new()`** plugin (`Deps = (WorkerSet, HealthRegistry)`):
  indicator `workers`, `UP` only while every worker is `Serving`.
- **`Mailboxes<M: Send + 'static>`** — cross-worker messaging, counted:
  `Mailboxes::new(set, capacity)` bean; inside a per-worker service
  `mail.attach(&ctx) -> Result<Mailbox<M>>` (`recv().await`, `try_recv`,
  `close`); from anywhere `send_to(i, m)`, `try_send_to(i, m) -> Result<(),
  (MailboxError, M)>`, `broadcast(m)` (`M: Clone`), `broadcast_with(|| m)`,
  `ask(i, |reply: oneshot::Sender<R>| m) -> Result<R>`, `ask_all(..) ->
  Vec<Result<R>>`. `MailboxError::{NoSuchWorker, NotAttached, Closed, Full,
  NoReply}`. Each send counts a `local`/`remote` crossing on the **target**.
- **Ingress affinity** (`r2e::runtime::ingress`): `reuseport_supported() ->
  bool` (const), `reuseport_tcp(addr) -> Result<std::net::TcpListener,
  AffinityError>`, `reuseport_udp(addr) -> Result<std::net::UdpSocket, _>`;
  `WorkerContext::adopt_udp(std) -> io::Result<rt::UdpSocket>` /
  `adopt_tcp_listener(std)` (must run on that worker's thread — asserts).
  `AffinityError::Unsupported { transport }` on platforms without
  `SO_REUSEPORT` — never a silent single-socket fallback. QUIC: pass the
  `reuseport_udp` socket to `quinn::Endpoint::new`.
- **`WorkerCollector`** (`r2e::r2e_prometheus`, feature `prometheus`):
  `Prometheus::builder().register(Box::new(WorkerCollector::new(set)))` →
  `r2e_workers`, `r2e_worker_state{worker,state}`, `r2e_worker_cpu{worker}`,
  `r2e_worker_crossings_total{worker,origin}`, `r2e_worker_mailbox_depth`,
  `r2e_worker_mailbox_sends_total`, `r2e_worker_mailbox_wait_seconds_total`
  (`with_namespace(set, "app")` to rename).
- **`WorkerHarness`** (tests): `WorkerHarness::start(n, vec![local.into_factory(),
  ..]).await?` boots `n` real worker threads (no listener, same barrier + reverse
  shutdown as serving); `run_on(i, |ctx| async { .. }) -> R` (future may be
  `!Send`), `run_on_all(..) -> Vec<R>`, `worker_set()`, `shutdown_token()`,
  `shutdown().await`. Proves instance count / execution worker / routing / drain
  deterministically.

Example: `examples/example-worker-udp` (shared-nothing UDP shards + control-plane
aggregation via `ask_all` every 10s, `/whoami`, `/stats`, `/workers`, `/metrics`).
