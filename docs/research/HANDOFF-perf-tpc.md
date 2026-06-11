# Handoff — Thread-per-core & performance work (r2e + proxy-mesh)

> Written 2026-06-11 to hand off to a fresh session. Self-contained: read this +
> `docs/research/thread-per-core.md` and you have the full picture. No prior chat needed.

## ⚡ Session 2 update (2026-06-11, later) — Wave 1 (r2e side) DONE

**Methodology: read `METHODOLOGY-subagent-implementation.md` FIRST and follow it** —
implementations are delegated to Opus 4.8 subagents (one per task, self-contained prompt),
the orchestrator audits the full diff and re-verifies. It caught 3 real defects in Wave 1;
the audit is not optional.

Completed and committed (tasks moved to closed in Tasker):

- **544 — `server.tcp_nodelay`**: config key (default `true`), applied via axum 0.8
  `ListenerExt::tap_io` in `PreparedApp::run_with_listener` (the single TCP serve path);
  `ListenerExt` re-exported r2e-http → `r2e_core::http`. Tests in
  `r2e-core/tests/tcp_nodelay.rs`. → proxy-mesh 539-master-part is now UNBLOCKED.
- **534 — `r2e_core::rt` facade**: `spawn` → opaque **`JobHandle<T>`** (the 534/535
  co-design decision is MADE: the opaque handle lives in `r2e_core::rt`, wraps tokio
  privately, exposes `abort`/`is_finished`/Future + opaque `JoinError`/`Elapsed`),
  `sleep`, `timeout`, `interval` (re-exports tokio `Interval`/`MissedTickBehavior`),
  `bind_tcp` (1:1 delegation to tokio bind — do NOT pre-resolve addresses, that was an
  audit-caught bug), `shutdown_signal` (extracted from builder.rs). All production spawn
  sites migrated; the 4 event backends now depend on r2e-core directly.
  Documented exceptions: `r2e-http/quic.rs` (below r2e-core in dep graph, quinn is
  tokio-bound — permanent), `r2e-core/src/lazy.rs` (runtime introspection, out of scope),
  and `PoolExecutor::spawn_job` (`r2e-executor/src/lib.rs` `// TODO(534/535)` — public API
  still returns `tokio::task::JoinHandle<T>`).
  Tests in `r2e-core/tests/rt.rs`. `rt` is reachable as `r2e::rt` via the facade glob.

- **535 — executor public API on `rt::JobHandle<T>`** (session 3, 2026-06-11): `submit`/
  `try_submit` return `Result<JobHandle<T>, RejectedError>`, `spawn_job` goes through
  `rt::spawn`, TODO(534/535) closed. `JobHandle`/`JoinError` stay in `r2e_core::rt`;
  r2e-executor re-exports them so `#[async_exec]` codegen emits `#exec_krate::JobHandle`.
  No `JoinError` extension was needed. Audit caught only cosmetic leftovers (stale
  `JoinHandle` doc comments in example-executor + builder.rs — the Wave-1 error profile
  holds: mechanical work fine, narrative omits residues). Commit 7debb27.

- **536 — SO_REUSEPORT sharded serving** (session 3, 2026-06-11): `server.workers: <n>|"per-core"`
  (cap 1024), new `r2e-core/src/sharded.rs`, lifecycle shared with the single path via an
  internal `ServeStrategy` enum + `run_inner` (no duplication), tcp_nodelay re-applied per
  worker, unix-only cfg mirroring socket2's `set_reuse_port` (excl. solaris/illumos/cygwin),
  dev-reload + sharding unsupported (warns, single listener). `rt` facade gained
  `spawn_blocking` and `lookup_host`. Implemented by an **Opus subagent** (user's choice) +
  orchestrator audit + an adversarial review pass — the adversarial pass caught 2 real
  defects the first audit missed: (1) first-address-only resolution (same bug class as the
  534 `bind_tcp` audit catch — fixed: all candidates tried for listener 0, remaining
  workers bind its `local_addr()`), (2) port 0 giving each worker a different ephemeral
  port (fixed by the same `local_addr()` mechanism, regression-tested). For delicate
  pieces the adversarial pass stays mandatory regardless of implementing model.
  Worker-death semantics documented (degraded capacity, no restart — v1 limitation).
  Commit 210eb15. Side discovery: `--features dev-reload` fails to build on master
  (pre-existing, beans.rs) → filed as task 549, fixed in 03735b5 (closed).

- **537 — control-plane / data-plane runtime split** (session 3, 2026-06-11): in sharded
  mode, background work initiated from request handlers runs on the caller's main
  multi-thread runtime (control plane), never on the worker `current_thread` runtimes.
  `rt` facade gained a **thread-local** control-plane handle (`set_control_plane`,
  registered by each worker thread; thread-local, not a global OnceLock, so multiple
  apps/runtimes per process — i.e. tests — cannot cross-spawn), `spawn_ctl` (routes onto
  it; byte-for-byte `spawn` when unset) and `current_handle`. Call-site classification:
  executor pool jobs + per-emit/unsubscribe event dispatch → `spawn_ctl`
  (handler-reachable); consumer loops, poller dispatch, scheduler/service/QUIC startup
  spawns stay `spawn` (already control-plane). lazy.rs worker-side first-touch resolves
  on the control plane via two-stage spawn + blocking channel (no `block_on` — it panics
  in async context; no hidden fallback runtime); NOTE(536→537) resolved. Opus subagent +
  audit + adversarial pass again: the pass flagged (1) docs hiding that a worker-side
  lazy first-touch stalls that worker's whole runtime (fixed: explicit caveat, eager
  resolution recommended), (2) factory panic payload swallowed (fixed: JoinError-based
  two-stage spawn re-raises the original payload on the worker, regression-tested),
  plus a warn when sharding is driven by a non-multi-thread runtime. One adversarial
  finding rejected as overstated (cross-thread cycle-detector claim — same-thread
  detection on the main runtime is unaffected; comment sharpened instead). Known v1
  limitation: cross-thread circular lazy deps deadlock instead of panicking with a
  trace (pre-existing with lazy-fallback-runtime). Commit b525cba.

- **538 — thread-per-core docs + benchmark** (session 4, 2026-06-11): extended the
  existing `docs/features/19-sharded-serving.md` (536/537 had already created it — no
  new file) with "When to use it", "Benchmark" and "Future directions" (options C/D
  preserved from the research doc); new `examples/example-sharded-bench` (/plain,
  /json, /db sqlite; mode switched at runtime via `R2E_SERVER_WORKERS` env overlay)
  + `tools/bench-sharded.sh` (oha, both modes, markdown table); CLAUDE.md routing
  row + features README index. Opus subagent + audit + adversarial pass. The audit's
  own re-run **invalidated the subagent's committed numbers**: its run happened under
  disk-full IO pressure and showed default winning 1.7–3.7× everywhere; three clean
  re-runs showed default ~10–30% ahead on /plain & /json with the ranking FLIPPING on
  one run, and only /db stable (~1.6× default). Table replaced with a clean run and
  the narrative rewritten around variance — macOS numbers are indicative only, the
  committed script is for the real Linux run (still TODO, table has an empty Linux
  block). Audit also fixed: hard-coded `/opt/homebrew/bin/oha` (script is *for*
  Linux re-runs), subshell PID semantics (`exec`), scratch/RESULTS_DIR leaks on
  early exit. Adversarial pass: `JobHandle` routing-keyword collision with
  executor.md (dropped), sharded-mode thread-count asymmetry now noted in
  methodology, unused deps removed. Commit 6c79bf5.

**r2e side of the plan is COMPLETE** (534, 535, 536, 537, 538, 544 all closed).
**Next up**: Wave 2/3 proxy-mesh items (541, 542, 540) in the proxy-mesh repo,
plus the deferred Linux benchmark run (fill the Linux table in
`docs/features/19-sharded-serving.md` via `tools/bench-sharded.sh`).

- **539 — proxy-mesh TCP_NODELAY** (session 5, 2026-06-12, proxy-mesh repo): the
  pre-implementation audit found the task already half-done — agent→target tunnel
  sockets (direct/brightdata/apify) all go through `configure_tunnel_socket()`
  (nodelay + keepalive), and reqwest 0.12 defaults `tcp_nodelay` to true, so only
  three gaps existed: (1) master listener → covered by bumping proxy-master's r2e
  rev ead533d → d314cd8 (server.tcp_nodelay default true, verified in the d314cd8
  source: applied via `tap_io` on the single-listener path used by `app.serve()`);
  (2) the **preauth listener** (`proxy-master/src/preauth.rs`, per-session CONNECT
  tunnels — a hot path the task description didn't list) → explicit
  `set_nodelay(true)` after accept; (3) agent→master WS →
  `connect_async_with_config(&url, None, true)` (tokio-tungstenite 0.26's
  `disable_nagle` param, verified in crate source: set on the raw TcpStream before
  the TLS wrap, so ws:// and wss:// both covered). QUIC is UDP, N/A. Opus subagent
  + orchestrator audit (clean — no defects found; adversarial pass skipped as the
  diff is 6 lines of mechanical change). Verification re-run by orchestrator:
  proxy-master 29/29, proxy-agent 6/6, proxy-e2e 9/9 (WS transport exercised),
  clippy warning count unchanged (20 before/after). Latency check before/after
  (2×30 runs each, loopback CONNECT tunnel): no measurable difference
  (~0.84ms p50 both) — expected, Nagle doesn't penalize single-write
  request/response on loopback; the win targets real links + multi-write patterns
  (TLS handshakes). Commit ec23aa7 (proxy-mesh).

## What this is

A research + planning conversation, **no code written yet**. We investigated how to push
performance in `r2e` (the framework) and `proxy-mesh` (a forward-proxy mesh built on top of
r2e), starting from the user's question: *can we support thread-per-core async runtimes
(monoio/compio/glommio) like ntex does, keeping ergonomics but allowing high performance and
zero-copy?*

Deliverables produced:
- `docs/research/thread-per-core.md` — the full research doc (10 sections). **Read it.**
- 11 Tasker tasks (6 on target `r2e`, 5 on target `proxy-mesh`), with dependency links.
- This handoff.

Nothing is committed yet — the two docs are untracked in the working tree.

## Key conclusions (the "why" behind the tasks)

1. **Thread-per-core (TPC) the ntex way is a full-stack rewrite** — axum/hyper/tower require
   `Send` futures + poll-based IO; monoio/glommio are `!Send` + completion-based (io_uring,
   owned buffers = the actual zero-copy enabler). ntex can do both because it owns its entire
   stack. r2e cannot, without rewriting it.
2. **Chosen path = Option A: SO_REUSEPORT sharding on tokio.** N worker threads, each a
   `current_thread` runtime with its own SO_REUSEPORT listener; kernel distributes connections.
   Keeps axum + the whole ecosystem. ~70-80% of the TPC benefit (scheduling), still epoll, no
   io_uring zero-copy. Opt-in via config (default unchanged).
3. **Other options**: B (axum on monoio via compat bridges) = rejected, copies buffers, kills
   the zero-copy point. C (pluggable HTTP engine, ntex path) = months, ecosystem split, only
   as a standalone project. D (hybrid io_uring data-plane for hot endpoints) = best ratio for
   *targeted* zero-copy, additive, future.
4. **Service/data sharing in sharded mode is unchanged** — state stays shared via `Arc` across
   cores (beans are `Clone+Send+Sync`). Sharding touches only accept loop + scheduling.
   Contention is the limit; future `#[inject(per_worker)]` scope is the escape hatch (still
   `Send` though — axum requires it; `!Send` is Option C territory).
5. **No-preemption reality**: async Rust can't interrupt a CPU-spinning `poll()`. Protections
   are detection (stall detector), offloading (`#[offload]`/`#[blocking]` via PoolExecutor),
   or topology (`threads_per_worker`, actix-style dispatch accept). The hard guarantee doesn't
   exist for anyone — document it honestly.
6. **io_uring**: do NOT take the tokio-uring crate (semi-maintained). Native tokio io_uring
   (behind `tokio_unstable` as of 1.47/1.48) is the path for fs ops — automatic win when stable.
   True network zero-copy (splice/sendfile/MSG_ZEROCOPY) needs owning the socket → Option D only.
   For a dedicated zero-copy data-plane, monoio > tokio-uring.
7. **proxy-mesh case study headline**: HTTP forward **buffers** (`HttpResult.body: Vec<u8>`,
   body inside a MessagePack message), while CONNECT tunnels already **stream** (`Bytes` over a
   dedicated QUIC stream). Aligning HTTP onto the tunnel model is the biggest architectural win
   there, independent of all TPC work. Note: master↔agent is QUIC (UDP userspace via quinn), so
   kernel splice does NOT apply on that hop — conscious QUIC-vs-splice trade-off.

## The tasks

### r2e (target `r2e`)
- **534** — `r2e_core::rt` facade centralizing tokio touchpoints (~10 spawn sites). Root of all TPC work.
- **535** — opaque `JobHandle<T>` replacing `tokio::task::JoinHandle` in r2e-executor + `#[async_exec]` codegen. **Breaking** (OK, not in prod).
- **536** — SO_REUSEPORT sharded serving, `server.workers: <n>|"per-core"`, N current-thread runtimes. Depends 534.
- **537** — control-plane / data-plane split (scheduler, services, events, lazy beans off the HTTP workers). Depends 536.
- **538** — docs + benchmark (sharded vs multi-thread). Depends 537.
- **544** — expose `server.tcp_nodelay` (set TCP_NODELAY on accepted connections). Relates 536, blocks proxy-mesh 539.

### proxy-mesh (target `proxy-mesh`)
- **539** — TCP_NODELAY on all hot-path sockets (client / master↔agent WS / agent↔target). Depends r2e 544 (for the client listener; agent part is independent).
- **540** — stream HTTP forward responses instead of buffering `body: Vec<u8>`. Depends 541 + 542.
- **541** — move HTTP body off the MessagePack path → raw-byte framing like tunnels.
- **542** — agent: stream upstream responses (`reqwest::bytes_stream()`/hyper) instead of full-body buffering.
- **543** — tunnel relay: try `copy_bidirectional` to skip the mpsc hop on QUIC streams. Measure-first.

## Implementation waves (the agreed order)

Within a wave = parallelizable; between waves = real dependency.

- **Wave 1 — immediate value, ~zero deps**: 544 (r2e tcp_nodelay), 539-agent-part (outbound socket nodelay), 534 (rt facade — start in parallel, it's the TPC root).
- **Wave 2 — foundations**: 535 (JobHandle — *co-design with 534*), 541 (protocol framing), 542 (agent streaming).
- **Wave 3 — big pieces**: 536 (sharding; must integrate 544), 540 (HTTP streaming master; *ship with 541+542*), 539-master-part (unblocked by 544).
- **Wave 4 — refinement**: 537 (control/data-plane split), 543 (tunnel copy_bidirectional, after 540, measure-first).
- **Wave 5 — close**: 538 (docs + benchmark; validates 536/540/544 retroactively).

Longest critical path: **534 → 536 → 537 → 538**.

## Coordination risks (NOT conflicts — but must be handled consciously)

1. **540 / 541 / 542 ship as ONE coordinated wire-protocol change** (single version bump).
   The `depends_on` links model dev order, not intermediate deploys — deploying 541 alone
   between a master and an agent breaks master↔agent compat mid-flight.
2. **534 + 535 co-design**: decide up front that `rt::spawn` returns `JobHandle<T>` and where
   that type lives, or you rewrite the signatures twice.
3. **544 before 536, and 536 must re-apply nodelay** in its new SO_REUSEPORT serve path.
4. **539 is not atomic**: agent-outbound part is doable now (Wave 1); master-listener part is
   blocked by 544 (Wave 3). Open question below.

Minor: do **543 after 540** (both touch the tunnel path; avoid reasoning about two refactors at once).

## Open questions for the user (raised, not yet answered)

- Split **539** into two tasks (agent / master) so the waves are clean?
- Add the coordination notes (esp. "ship 540/541/542 as one block") directly into the Tasker
  task descriptions?
- File the remaining perf axes as tasks (currently backlog-only in the research doc §8):
  SIMD JSON feature flag, build-controller-once, validated-token cache, moka cache backend,
  hot-path allocation audit, minimal-telemetry mode; plus the native-tokio-io_uring watch item (§9).
- Commit the two research docs?

## How to resume in the fresh session

1. Read `docs/research/thread-per-core.md` (full analysis) + this file +
   `METHODOLOGY-subagent-implementation.md` (how to run the implementation waves).
2. Tasker tasks live on targets `r2e` and `proxy-mesh` (ids 534-544). Use the Tasker MCP
   (`list_tasks target:r2e` / `target:proxy-mesh`) to see current state.
3. arcle has both repos indexed (`r2e`, `proxy-mesh`) — sync before exploring.
4. Likely first concrete work: ~~Wave 1~~ done (see Session 2 update) — next is **535**
   (small: executor public API onto `rt::JobHandle<T>`), then **536** (sharding, delicate).
5. Codebase anchor points already located:
   - r2e serve path: `r2e-core/src/builder.rs:1406` (serve) / `:1614` (axum serve + make_service).
   - tokio spawn sites: builder.rs, r2e-events (local.rs + backend/state.rs + 4 backends),
     r2e-executor/lib.rs, r2e-scheduler/types.rs, r2e-http/quic.rs.
   - runtime-flavor assumption: `r2e-core/src/lazy.rs::resolve_lazy_factory`.
   - per-request controller clone: `r2e-macros/src/derive_codegen.rs:159/285`.
   - `#[async_exec]` JoinHandle codegen: `r2e-macros/src/codegen/wrapping.rs`.
   - proxy-mesh buffering gap: `proxy-master/src/responses.rs` (`HttpResult.body: Vec<u8>`).
   - proxy-mesh tunnel relay: `proxy-master/src/transport/quic.rs` (`read_tunnel_recv`,
     `run_tunnel_writer`, TUNNEL_WRITER_CAPACITY=256).
