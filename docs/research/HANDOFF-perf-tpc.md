# Handoff — Thread-per-core & performance work (r2e + proxy-mesh)

> Written 2026-06-11 to hand off to a fresh session. Self-contained: read this +
> `docs/research/thread-per-core.md` and you have the full picture. No prior chat needed.

## ⚡ Session 2 update (2026-06-11, later) — Wave 1 (r2e side) DONE

**Methodology: read `METHODOLOGY-subagent-implementation.md` FIRST and follow it** —
implementations are delegated to Sonnet subagents (one per task, self-contained prompt),
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

**Next up**: 536 (SO_REUSEPORT sharding — delicate piece: orchestrator implements directly
or adds an adversarial review pass; it must re-apply tcp_nodelay in its accept path, see
task notes). Wave 2 proxy-mesh items (541, 542) are in the other repo.

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
