# Research: Thread-per-core runtime support (monoio / compio / glommio)

> Status: research notes, June 2026; pruned 2026-07-23 (shipped items removed).
> Option A (SO_REUSEPORT sharding, tasks 534–538 + 544) is **implemented** — see
> `docs/features/19-sharded-serving.md`. What is left here is design reference plus the
> unbuilt backlog (§6, §7, §8, §9).
> Goal: support thread-per-core (TPC) async runtimes like ntex does, while keeping R2E's
> ergonomics (controllers, DI, guards) and opening a path to high performance / zero-copy IO.

## TL;DR

The conflict is structural. R2E delegates all HTTP to axum/hyper/tower, which require `Send`
futures and a **poll-based** (readiness) IO model. monoio/glommio are the exact opposite:
`!Send` futures and **completion-based** IO (io_uring, owned buffers — which is what enables
zero-copy). ntex can support both because it owns its *entire* stack (ntex-rt, ntex-io,
ntex-service, its own HTTP codec). R2E does not.

Realistic options range from "socket sharding on tokio" (cheap, big win) to "replace the HTTP
engine" (that is rewriting ntex). Decision: **do option A now** (SO_REUSEPORT sharding),
**option D when a concrete zero-copy need appears**, **option C only as a standalone project**.

---

## 1. The fundamental conflicts

| Axum/hyper/tower world | monoio/glommio world |
|---|---|
| Futures must be `Send` (work-stealing) | Futures are `!Send` (`Rc`, per-core driver handles) |
| Poll-based IO: `AsyncRead`/`AsyncWrite` borrow buffers | Completion-based IO: io_uring needs **owned** buffers (`IoBuf`) |
| Shared state (`Arc`), one bean graph | Per-core state (`Rc`), state factory per worker |
| tower `Service` (Send bounds) | Needs a `!Send`-friendly service trait (ntex-service) |

- The completion→poll compat bridges (monoio `poll-io`, compio hyper bridges) force a buffer
  copy on every IO — they exist, but they destroy precisely the zero-copy benefit that
  motivated the change.
- compio is completion-based but cross-platform (IOCP on Windows); glommio is Linux-only
  io_uring; monoio has both an io_uring driver and an epoll/kqueue legacy driver.
- The key design trick that makes ntex runtime-generic: **ntex-io exposes a poll-style API
  with internally-owned buffers**, implementable on both epoll and io_uring. That is the
  hardest piece to design and the one to study first if option C is ever pursued
  (codebases to read: `ntex-io`, `xitca-web`).

## 2. Residual tokio coupling (post-534/537 — what is left)

The `r2e_core::rt` facade (`r2e-core/src/rt.rs`) now owns every production spawn/sleep/
timeout/bind touchpoint, so the survey that used to live here is obsolete. What remains
coupled, deliberately:

- **`r2e-http/src/quic.rs`** calls `tokio::spawn` directly — it sits below r2e-core in the
  dep graph and quinn/h3 are tokio-bound. Permanent exception.
- **`r2e-core/src/lazy.rs`** introspects the runtime flavor (`block_in_place` on multi-thread,
  control-plane routing on sharded workers). Out of scope for the facade.
- **`SseBroadcaster` / `WsBroadcaster`** document `tokio::sync::broadcast` semantics (lag,
  `len`, `is_empty`) in their public contract (`r2e-core/src/ws.rs`, `sse.rs`). A doc/semantics
  coupling only — broadcast works fine on `current_thread`. **Not addressed.**
- **Distributed event backends** (rdkafka, lapin, pulsar, iggy) are tokio-bound client libs.
  They stay on tokio in *every* scenario; in a TPC architecture they live on the control-plane
  runtime, never in HTTP workers.
- `tokio::sync` primitives (broadcast, Notify, Semaphore, RwLock) are runtime-agnostic and
  out of scope for any abstraction.

## 3. The options

### Option A — SO_REUSEPORT sharding on tokio (CHOSEN — SHIPPED)

N threads, each with its own `current_thread` tokio runtime and its own listener bound with
SO_REUSEPORT. The kernel distributes connections; zero work-stealing, zero cross-core synchro
on the accept path, good cache locality. Same spirit as ntex's default worker model and nginx.

- Keeps axum, tower, the whole ecosystem, and every R2E feature.
- ~70–80 % of the TPC benefit (the scheduling part), but still epoll and no io_uring zero-copy.
- Handlers stay `Send` (axum requires it regardless).
- Config: `server.workers: <n> | "per-core"`, absent = current behavior (opt-in!).

Shipped in `r2e-core/src/sharded.rs`; kept here for the rationale behind B/C/D below.

### Option B — compat bridges (axum/hyper on monoio/compio) — REJECTED

Possible (monoio `poll-io` mode, compio hyper bridges) but the completion→poll bridge copies
buffers, axum still requires `Send`, so no `Rc` in handlers. You pay the complexity of an
exotic runtime and lose exactly the zero-copy you came for. Net gain ≈ zero.

### Option C — pluggable HTTP engine (the ntex path) — only as a dedicated project

R2E's real asset: the user-facing surface is **macro-generated**. `#[routes]` emits axum
handlers today; it could emit handlers for another engine behind a feature flag. The user
model (controllers, guards, DI) could survive. But the replacement list is brutal:

- An own `Service` trait without mandatory `Send` (tower is unusable in `!Send`).
- Own extractors (`FromRequestParts` is an axum trait).
- **Per-core state model**: `StatefulConstruct` called once per worker, `#[inject]`
  accepting `!Send` types — ntex's factory pattern (`App::new(|| ...)`).
- Guards/interceptors as `LocalBoxFuture`.
- An HTTP/1.1 (and h2?) codec on the target runtime.
- **Ecosystem split**: sqlx, rdkafka, lapin, quinn are tokio-bound. A monoio backend cannot
  use r2e-data-sqlx, the event backends, or current QUIC without cross-runtime bridges
  (channels to a side tokio runtime — what people do in practice, with the latency that implies).

Months of work. Essentially building ntex inside R2E.

### Option D — hybrid data-plane — best ratio for targeted zero-copy

Keep tokio/axum as control plane (REST, auth, config, events…), and offer TPC io_uring
listeners for specific hot endpoints: a `RawService`-style trait mounted on dedicated monoio
cores, communicating with the rest of the app via channels or lock-free state. Zero-copy only
pays on specific paths (proxying large payloads, streaming, files) — not on 2 KB JSON where
serialization dominates anyway. Additive, does not touch the architecture.

### Reality check on benchmarks

For most HTTP workloads (handlers doing SQL, JSON, network calls), TechEmpower-style
benchmarks flatter TPC far more than real production. SO_REUSEPORT sharding + `Bytes`
everywhere (`send_binary` already widened to `impl Into<Bytes>`) + sendfile for static
files already gets very close to the ceiling reachable without rewriting the IO stack.

## 4. Service / data sharing model in sharded mode

**With option A, nothing changes — all state stays shared via `Arc` across cores.**
Sharding touches only the accept loop and the scheduling, not the data model:

- State (`Services`) is built **once** on the control plane; beans are `Clone + Send + Sync`
  (`Arc<Inner>` clones). The axum router is cloned per worker, the state with it.
- `#[inject] user_service: UserService` → all workers see the *same instance* (one sqlx
  pool, one `JwksValidator`, one `TtlCache` shared by N cores).
- Local EventBus → one bus. An event emitted from worker 3 reaches a `#[consumer]` on the
  control plane. Scheduler/ServiceComponents inject the same shared beans.

**The limit: sharing is contention.** What share-nothing TPC (ntex, ScyllaDB) eliminates is
exactly that: a hot `RwLock`/`DashMap` touched by all cores → cache-line bouncing. Remedies,
in increasing effort:

1. **Shard the bean internally** — e.g. a `TtlCache` holding N sub-caches indexed by worker
   id. Invisible to callers, no framework change.
2. **Lock-free / append-only structures** — runtime-independent.
3. **A real per-core scope in R2E** — natural evolution if benchmarks confirm contention:

   ```rust
   #[controller]
   pub struct HotController {
       #[inject] db: DbPool,                    // app-scoped: shared, Arc
       #[inject(per_worker)] cache: LocalCache, // built N times, one instance per core
   }
   ```

   Semantics: `StatefulConstruct` called once *per worker* at boot. **Important nuance**:
   even sharded, axum requires `Send` handlers, so per-worker types must still be `Send`
   (no `Rc`/`RefCell`) — they never actually cross threads, but the type system doesn't
   know that. You gain non-contention, not `!Send` ergonomics (that is option C territory).

**Share-nothing data between workers** (if ever pursued): with SO_REUSEPORT the *kernel*
picks the core for a connection (hash of the TCP 4-tuple) — you cannot route a request to
the core that owns the data. Known patterns: selective sharing (per-core by default, `Arc`
for what must be global — the pragmatic hybrid), message passing between workers (mpsc
forward to the owning core; reintroduces inter-core latency), client-side partition routing
(ScyllaDB model; not applicable to generic HTTP). **Build none of this now.**

## 5. Why axum doesn't do this by default

Deliberate philosophy, not an oversight. `axum::serve` = one listener, one accept loop;
each connection is spawned onto the multi-thread runtime and **work-stealing** distributes.

- **Work-stealing (tokio default)**: great for heterogeneous load. A slow request or an
  unlucky core gets absorbed by idle workers. Cost: queue synchronization, task migration
  (hence `Send` everywhere), degraded cache locality.
- **TPC**: zero synchro, perfect locality. Cost: a connection is **pinned** to its accepting
  core forever. If a core draws bad connections (2 GB stream, CPU-bound handler),
  everything pinned there waits — no rebalancing will come. The classic TPC footgun, and
  the reason tokio made it opt-out rather than default.

TPC wins on **short, homogeneous** requests (the TechEmpower profile — hence ntex/actix
numbers); it loses on long-tail workloads. Hence `server.workers` is opt-in.
Nothing *prevents* doing it with axum (bind N SO_REUSEPORT sockets yourself, run N
`axum::serve` on N current_thread runtimes) — it is just ~60 lines of socket2 + threads +
shutdown plumbing that R2E turns into one config line. actix-web and ntex have run N
single-threaded workers by default forever; tokio/axum simply chose the more robust default.

## 6. TPC with protection against long tasks

**The unavoidable constraint: async Rust has no preemption.** A `poll()` spinning on CPU
cannot be interrupted — not by tower timeouts (cancellation only acts at `await` points),
not by the runtime. The only real preemption is the OS's, i.e. threads. ntex and glommio
have the same limit (glommio only offers a manual `yield_if_needed()`). So every
"protection" is either **detection**, **offloading**, or **topology**:

### Layer 1 — Detection: stall detector (near-free, do by default)

A heartbeat per worker: a task arming every ~100 ms measuring its wake-up delay. Over a
threshold → warn log + `r2e_worker_loop_lag_seconds{worker="N"}` metric via r2e-prometheus
(tokio-metrics "slow poll ratio" pattern). Doesn't protect, but turns the silent footgun
into a visible alert in dev, and tells you *which* core suffers in prod.

### Layer 2 — Offloading: R2E already has the right piece

`PoolExecutor` (r2e-executor) is the escape hatch: a bounded pool on the control-plane
runtime. Missing piece is route-level ergonomics:

```rust
#[get("/report")]
#[offload]            // body runs on the PoolExecutor;
async fn heavy(&self) -> Json<Report> { ... }  // the worker only awaits the result
```

Plus `#[blocking]` → `spawn_blocking` for synchronous blocking code (works fine from a
current_thread runtime; the blocking pool is separate). Envoy/nginx model: event loops are
sacred, compute goes elsewhere. Fits the macro system naturally.

### Layer 3 — Topology: bound the worst case

- **`threads_per_worker: 2`** — N/2 runtimes × 2 threads instead of N × 1. Work-stealing
  only *within* a pair: ~90 % of the locality kept, but a long task can never freeze a
  whole silo — its partner absorbs. Trivial to offer (one runtime-builder parameter), and
  compatible since option A keeps `Send` everywhere.
- **Dispatch accept (actix-server model)** — one accept thread hands connections to the
  **least loaded** worker; a saturated worker stops receiving new ones. Doesn't unpin
  existing connections but stops piling new ones onto a suffering core. Cost: one channel
  hop per connection (amortized over keep-alive). Keep in backlog until SO_REUSEPORT hash
  imbalance is proven in practice.

**Recommended combo**: stall detector by default + `#[offload]`/`#[blocking]` + optional
`threads_per_worker`. And be honest in the docs: an un-annotated handler spinning on CPU
without `await` can still freeze its core — the hard guarantee does not exist in async
Rust, for anyone.

## 7. Backlog (nothing here is implemented)

Tasks 534–538 and 544 are done (see `HANDOFF-perf-tpc.md`). Never filed, never built:
stall detector, `#[offload]` / `#[blocking]` route attributes, `threads_per_worker` config,
`#[inject(per_worker)]` scope, actix-style dispatch accept, option D raw io_uring
data-plane listeners. All are described in §6 and §3 above.

## 8. Other performance axes (beyond thread-per-core) — none of these are built

Ordered by gain/effort. **Do #8.0 first — without it, everything else is guesswork.**

**8.0 Continuous benchmark harness (prerequisite).** criterion microbenches on the generated
path (dispatch → DI extract → guards → handler → serialization) + a macro bench (oha) in CI to
catch regressions. `tools/bench-sharded.sh` + `examples/example-sharded-bench` (task 538) cover
the sharded-vs-default macro case only — widen to a permanent harness.

Quick wins (days):
1. **SIMD JSON behind a feature flag** — `r2e::Json<T>` switching to sonic-rs behind
   `feature = "fast-json"` (drop-in), typically ×2–3 on (de)serialize. R2E owns the re-exported
   `Json` type, so this is exactly framework-packageable value.

Medium (weeks):
2. **Validated-token cache in r2e-security** — JWKS is cached, but signature verification (RSA
   especially) is paid per request — often the #1 CPU cost of an authenticated API. A bounded
   LRU `hash(token) → claims` keyed by token `exp` removes it (revocation window = cache TTL,
   document honestly).
3. **moka backend for the cache** — `CacheStore` is already a pluggable trait; a moka backend
   (lock-free, TinyLFU) makes the `Cache` interceptor + `TtlCache` far less contended under
   load. Converges with the TPC "hot shared bean" topic.
4. **Hot-path allocations** — request-id still allocates a `String` per request
   (`r2e-core/src/request_id.rs`: UUID v4 + `to_string`; the double String+HeaderValue alloc
   was already removed, the id itself was not) — go stack-encoded, or a faster id. Plus a
   `HeaderValue::from_static` / pre-allocated capacity audit on plugin-built responses.
5. **Default observability overhead** — per-request tracing span + Prometheus histogram cost
   more than expected (~µs/req, matters at 100k rps). Offer a "minimal telemetry" mode:
   level-gated spans, reduced histogram buckets, no eager span-field evaluation when filtered.

> Dropped as done: TCP_NODELAY on accepted connections (task 544, `server.tcp_nodelay`);
> build-controllers-once (the controller core is now built once into an `Arc` at registration,
> one clone per request); EventBus `Arc<EventMetadata>` + lazy construction.

## 9. io_uring / zero-copy for specific features

**Do not take the tokio-uring crate dependency.** It's in semi-maintenance (changelog stale
since 2022, rare releases); the tokio team's official direction is to integrate io_uring into
tokio *itself*. As of tokio 1.47/1.48 (2025), io_uring is behind `tokio_unstable` — `fs::write`
and `OpenOptions::open` use it when enabled, with the goal of transparently swapping the file
API backend. **→ backlog: enable native-tokio io_uring for fs ops behind an r2e feature flag
once it leaves unstable. Automatic win for multipart and any future file IO, zero arch change.**

Disambiguate "zero-copy":
- **io_uring on fs ops ≠ zero-copy** — eliminates syscalls + the `spawn_blocking` detour, but
  data still transits userspace. A file throughput/latency win, not zero-copy.
- **True network zero-copy** = `sendfile`/`splice` (disk→socket, no userspace) or `MSG_ZEROCOPY`
  — requires *owning the socket*, which hyper never allows. So that's option D: dedicated
  data-plane listeners outside axum. With TLS it also needs kTLS (else userspace crypto
  reintroduces the copy).

Per-feature verdict: `r2e-static` (rust_embed, already in-memory `Bytes`) — nothing to gain;
multipart→disk — yes via native-tokio io_uring when stable; large downloads/payload proxying —
true zero-copy only in option D (splice/sendfile + kTLS); binary WS/SSE — already shared
`Bytes`, `MSG_ZEROCOPY` only on very large frames (marginal). For a dedicated zero-copy
data-plane in 2026, **monoio** is the better runtime than tokio-uring (active, more mature
io_uring usage; its tokio-interop edge doesn't matter for an isolated channel-fed data-plane).

## 10. Case study — proxy-mesh (built on r2e)

proxy-mesh is the ideal workload to validate all of the above: Master (HTTP/CONNECT forward
proxy) ⟷ QUIC/WS ⟷ Agent (exit node). Pure network relay, no DB/CPU on the hot path — exactly
where TPC and zero-copy pay.

The headline finding — *HTTP forward buffered while CONNECT tunnels already streamed* — has
been **fixed**: tasks 541 (body off MessagePack → raw framing, protocol v2), 542 (agent streams
both directions) and 540 (master streams both directions) shipped as one wire block, along with
539 (TCP_NODELAY on all hot-path sockets). See `HANDOFF-perf-tpc.md` for the deploy gate.

Still open there:

- **543 — tunnel relay via `copy_bidirectional`** to skip the mpsc hop on QUIC streams
  (`read_tunnel_recv` → channel cap 256 → `run_tunnel_writer`). **Measure-first.** The
  `try_send→Full→spawn` chunk-reorder hazard on tunnels belongs to this task.
- **r2e sharding (536) applied to proxy-mesh** — the framework side is shipped; proxy-mesh has
  not been switched to `server.workers` / benchmarked under it.
- **Kernel zero-copy (option D, future)** — splice/sendfile on tunnels, BUT master↔agent is
  QUIC (UDP userspace via quinn), so splice does not apply on that hop; only agent↔target
  (if TCP) would benefit. QUIC (resilience, 0-RTT, no HoL) vs splice (zero-copy) is a
  conscious trade-off. Only after benchmarks.
