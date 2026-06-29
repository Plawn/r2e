# Research: Thread-per-core runtime support (monoio / compio / glommio)

> Status: research notes, June 2026. Implementation tracked in Tasker tasks 534–538 (target `r2e`).
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

## 2. Where R2E is coupled to tokio today (arcle survey, snapshot 540 @ `ead533d`)

### `tokio::spawn` production call sites (~10, well localized)

| Crate | Sites | Role |
|---|---|---|
| `r2e-core/src/builder.rs` | 2 | `ServiceComponent` start + QUIC server |
| `r2e-events` (local.rs + backend/state.rs) | 5 | handler dispatch, unsubscribe |
| `r2e-events/backends/{iggy,kafka,pulsar,rabbitmq}` | 2 each | pollers/consumers |
| `r2e-executor/src/lib.rs` | 2 | `PoolExecutor` core |
| `r2e-scheduler/src/types.rs` | 1 | scheduled task loop |
| `r2e-http/src/quic.rs` | 1 | per-request h3 spawn |

### Public-API leaks of tokio types (the two that matter)

1. **`r2e-executor` exposes `tokio::task::JoinHandle` directly**, and the `#[async_exec]`
   codegen (`r2e-macros/src/codegen/wrapping.rs`) hardcodes
   `::tokio::task::JoinHandle<#return_ty>` in generated signatures. Needs an opaque
   `JobHandle<T>` wrapper (breaking change — acceptable, not in production). → task 535
2. **`SseBroadcaster`/`WsBroadcaster` document `tokio::sync::broadcast` semantics** (lag,
   `len`, `is_empty`) in their public contract. Less severe: broadcast works fine on
   current_thread; it is a doc/semantics coupling only.

### Runtime-flavor assumptions

- `r2e-core/src/lazy.rs::resolve_lazy_factory` is the **only** place that introspects the
  runtime flavor: `block_in_place` only on multi-thread, otherwise falls back to a hidden
  `OnceLock` multi-thread fallback runtime. In sharded current-thread workers every lazy
  bean resolution would silently go through that fallback runtime — works, but must be
  handled explicitly (route to the control-plane runtime instead). → task 537

### Useful existing mechanics

- `r2e-core/src/dev.rs` already does `std listener try_clone → from_std` for hot-reload.
  That is exactly the mechanism needed for SO_REUSEPORT sharding (socket2 to set the
  option, then per-worker `from_std`).
- `#[r2e::main]` already supports `flavor = "current_thread"` (`r2e-macros/src/main_attr.rs`).
- Shutdown already uses `CancellationToken` (tokio-util) everywhere — broadcasts to N
  workers as-is.
- Serve path: `r2e-core/src/builder.rs:1406` (`serve`) → `:1614`
  (`crate::http::serve` + `into_make_service_with_connect_info`), listener =
  `tokio::net::TcpListener::bind`.
- Distributed event backends spawn via tokio-bound client libs (rdkafka, lapin, pulsar,
  iggy). They stay on tokio in **every** scenario — in a TPC architecture they live on a
  side/control-plane runtime, never in HTTP workers.
- `tokio::sync` primitives (broadcast, Notify, Semaphore, RwLock — e.g. `r2e-security/src/jwks.rs`)
  are runtime-agnostic and out of scope for any abstraction.

## 3. The options

### Option A — SO_REUSEPORT sharding on tokio (CHOSEN, tasks 534–538)

N threads, each with its own `current_thread` tokio runtime and its own listener bound with
SO_REUSEPORT. The kernel distributes connections; zero work-stealing, zero cross-core synchro
on the accept path, good cache locality. Same spirit as ntex's default worker model and nginx.

- Keeps axum, tower, the whole ecosystem, and every R2E feature.
- ~70–80 % of the TPC benefit (the scheduling part), but still epoll and no io_uring zero-copy.
- Handlers stay `Send` (axum requires it regardless).
- Config: `server.workers: <n> | "per-core"`, absent = current behavior (opt-in!).

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
numbers); it loses on long-tail workloads. Hence `server.workers` is opt-in in task 536.
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

## 7. Implementation plan (Tasker, target `r2e`)

| # | Task | Priority | Depends on |
|---|---|---|---|
| 534 | `r2e_core::rt` facade centralizing tokio touchpoints | high | — |
| 535 | Opaque `JobHandle<T>` in r2e-executor + `#[async_exec]` codegen (breaking) | high | relates to 534 |
| 536 | SO_REUSEPORT sharded serving — `server.workers` config, N current-thread runtimes | high | 534 |
| 537 | Control-plane / data-plane split (scheduler, services, events, lazy beans) | medium | 536 |
| 538 | Docs + benchmark sharded vs multi-thread | low | 537 |

Natural order: 534 → 535 ∥ 536 → 537 → 538.

Backlog candidates (not yet filed): stall detector (could fold into 536), `#[offload]` /
`#[blocking]` route attributes, `threads_per_worker` config, `#[inject(per_worker)]` scope,
actix-style dispatch accept, option D raw io_uring data-plane listeners.

Also filed (surfaced by the proxy-mesh case study, §10):
- **544** — `r2e-core`: expose `server.tcp_nodelay` (set TCP_NODELAY on accepted connections).
  Relates to 536, blocks proxy-mesh 539.

## 8. Other performance axes (beyond thread-per-core)

Verified against the code (`derive_codegen.rs`, `response.rs`, `request_id.rs`).
Ordered by gain/effort. **Do #8.0 first — without it, everything else is guesswork.**

**8.0 Continuous benchmark harness (prerequisite).** criterion microbenches on the generated
path (dispatch → DI extract → guards → handler → serialization) + a macro bench (oha) in CI to
catch regressions. Partly covered by task 538 — should be widened to a permanent harness.

Quick wins (days):
1. **TCP_NODELAY on accepted connections** — `axum::serve` doesn't set it; Nagle + delayed-ACK
   can add ~40ms on small responses. Default `true` + config key. → **filed as task 544.**
2. **SIMD JSON behind a feature flag** — `r2e::Json<T>` switching to sonic-rs behind
   `feature = "fast-json"` (drop-in), typically ×2–3 on (de)serialize. R2E owns the re-exported
   `Json` type, so this is exactly framework-packageable value.
3. **Build controllers once, not per request** — the generated extractor does N `Arc::clone` +
   struct construction per request (`derive_codegen.rs:159/285`). For controllers with NO
   identity fields, the derive could build the instance once at boot and share an
   `Arc<Controller>` → one clone per request instead of N. Identity controllers keep the
   current path. Localized change, invisible to users.

Medium (weeks):
4. **Validated-token cache in r2e-security** — JWKS is cached, but signature verification (RSA
   especially) is paid per request — often the #1 CPU cost of an authenticated API. A bounded
   LRU `hash(token) → claims` keyed by token `exp` removes it (revocation window = cache TTL,
   document honestly).
5. **moka backend for the cache** — `CacheStore` is already a pluggable trait; a moka backend
   (lock-free, TinyLFU) makes the `Cache` interceptor + `TtlCache` far less contended under
   load. Converges with the TPC "hot shared bean" topic.
6. **Hot-path allocations** — request-id (UUID v4 + `to_string` per request, `request_id.rs:64`
   — go stack-encoded everywhere, or a faster id), EventBus metadata (already tracked tech debt:
   Arc metadata, lazy `EventMetadata::new`), and a `HeaderValue::from_static` / pre-allocated
   capacity audit on plugin-built responses.
7. **Default observability overhead** — per-request tracing span + Prometheus histogram cost
   more than expected (~µs/req, matters at 100k rps). Offer a "minimal telemetry" mode:
   level-gated spans, reduced histogram buckets, no eager span-field evaluation when filtered.

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
where TPC and zero-copy pay. Data path has two very different regimes (arcle survey, snapshot
552):

- **CONNECT tunnel (HTTPS)**: dedicated QUIC bidi stream per tunnel, raw bytes, `Bytes`
  throughout, relayed via mpsc (`read_tunnel_recv` → channel cap 256 → `run_tunnel_writer`).
  Already good — 8-byte `RequestId` header + raw bytes, *off* the MessagePack path.
- **HTTP forward**: `HttpResult.body: Vec<u8>` (`proxy-master/src/responses.rs`) — body **fully
  buffered** AND carried *inside* a MessagePack message. TTFB = full response time; peak memory
  = body size per in-flight request. The biggest architectural gap.

Optimizations by implementation layer:

| Layer | Opportunity | Tasker |
|---|---|---|
| **r2e (framework)** | SO_REUSEPORT sharding (proxy = ideal TPC workload, no contended hot-path state); TCP_NODELAY config | 536, 544 |
| **proxy-master** | Stream HTTP responses (align with tunnels); TCP_NODELAY on client sockets; tunnel relay via `copy_bidirectional` to skip the mpsc hop | 540, 539, 543 |
| **transport / proxy-protocol** | Move HTTP body off MessagePack → raw framing like tunnels; reuse `BytesMut` in codec | 541 |
| **proxy-agent** | Stream upstream via `reqwest::bytes_stream()`/hyper instead of full-body buffer; TCP_NODELAY on outbound; TPC (same profile as master) | 542, 539 |
| **kernel zero-copy (option D, future)** | splice/sendfile on tunnels — BUT master↔agent is QUIC (UDP userspace via quinn), so splice doesn't apply there; only agent↔target (if TCP) would benefit. QUIC (resilience, 0-RTT, no HoL) vs splice (zero-copy) is a conscious trade-off | — |

**Headline finding: HTTP forward buffers, tunnels already stream.** Aligning the former on the
latter is the single biggest architectural win in proxy-mesh, and it's independent of all the
TPC/r2e work. Recommended order: (1) TCP_NODELAY everywhere — cheapest latency win; (2) HTTP
response streaming (master+protocol+agent together, 540/541/542); (3) r2e sharding (536) when
ready — proxy benefits directly; (4) `copy_bidirectional` on tunnels (543, measure-first);
(5) kernel zero-copy only after benchmarks, accepting the QUIC trade-off.

proxy-mesh Tasker tasks: 539 (TCP_NODELAY), 540 (HTTP streaming), 541 (body off MessagePack),
542 (agent streaming), 543 (tunnel copy_bidirectional). Dependencies: 540 → {541, 542};
539 → r2e 544 → relates r2e 536.
