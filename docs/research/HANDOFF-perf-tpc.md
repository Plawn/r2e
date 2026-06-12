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

- **541 — HTTP body off MessagePack, raw framing + protocol v2** (session 5, 2026-06-12,
  proxy-mesh repo, commit 1b2cd68 — **LOCAL ONLY, do not push**: 540/541/542 ship as one
  block, and a master push triggers GitLab CI auto-deploy). The full coordinated wire
  change was designed up front (`/tmp/541-design/DESIGN-wire-http-body.md`, amended by an
  adversarial design review — 8 amendments integrated) so 542/540 need no further wire
  changes. What landed:
  - Raw frame tags 0x04/0x05/0x06 (`HttpBodyChunk/End/Abort`, both directions, never
    msgpack); `ProxyHttp.has_body`; `HttpResponseStart` replaces `HttpResponse` (body
    always framed, empty = immediate END); `MAX_HTTP_BODY_SIZE` = 64MiB.
  - **PROTOCOL_VERSION=2 handshake**: `Register.protocol` (serde-default 0), master Kicks
    mismatched agents, and sends `Welcome{protocol}` FIRST (rmp_serde ignores unknown
    fields → the Register gate alone is one-directional); agent requires Welcome within
    5s else disconnect+retry. **Deploy masters before agents.**
  - QUIC: master-opened bi-stream per HTTP request (9-byte header `[0x01][id u64 BE]`);
    tunnel streams untouched (8-byte header). Removes the implicit 16MiB QUIC response
    cap (e2e-verified with a 20MiB body). Pending buffers both directions for
    control-frame/stream races; `HttpAccumulator.pending_end` for start/end reorder.
  - `ResponseRegistry.fail_node()` at both deregister points — fixes a pre-existing bug
    where agent death left in-flight HTTP requests hanging until timeout.
  - preauth forward path refactored through `execute_http_on_node` (single ProxyHttp
    emitter, metrics parity).
  - Process: Opus subagent implementation + orchestrator line-by-line audit (caught a
    missing Welcome 5s timeout the subagent's report didn't flag) + adversarial
    implementation review, which produced **4 refuted findings, all fixed**: (1) QUIC
    `open_bi` awaited in the session select loop = up-to-5s head-of-line block → moved
    off-loop (writer registered synchronously, open+header+writer as one task); (2)
    ingress 64MiB cap only enforced AFTER full `collect()` on all 3 master paths →
    `collect_body_capped()` errors mid-collection (413); (3) the tunnel
    try_send→Full→spawn pattern REORDERS chunks under backpressure and was copied onto
    HTTP paths → HTTP writer/body channels are now **unbounded** (ordering guaranteed;
    memory bounded by the 64MiB cap + refcounted `Bytes` slices, zero-copy chunking both
    sides) — tunnels still have the reorder hazard, deliberately left to 543; (4) agent
    pending-body flush could silently drop the terminal `End` (hung request task) →
    unbounded channel cannot refuse.
  - e2e: harness builds binaries; local HTTP server with **position-dependent payloads**
    (`i % 251` — uniform fills can't detect chunk reorder); 1MiB POST + 1.5MiB GET on
    WS+QUIC + 20MiB QUIC GET. Verification (orchestrator re-run): units 6+39+20, e2e
    **24/24** (needs `cargo test -p proxy-e2e --features quic` — quic is non-default
    there; without it only 11 WS tests run).
  - Deferred (acceptable, noted for later): unbounded pending maps for misbehaving-master
    ids, agent-side independent body cap, per-request agent timeout (agent task can wait
    forever on a body that never comes if the master dies mid-request — master side
    resolves via fail_node). For 542: the master QUIC code no longer removes the http
    writer when the reader joins, but re-examine writer lifecycles when the agent can
    respond before the request body ends.

- **542 — agent streams HTTP both directions** (session 5, 2026-06-12, proxy-mesh repo,
  commit 45cda55 — **LOCAL ONLY, do not push**, same block rule). No wire change,
  master untouched, PROTOCOL_VERSION still 2. What landed:
  - `ExitStrategy::execute_http` now takes `HttpRequestStart { method, url, headers,
    body: Option<reqwest::Body> }` and returns `HttpResponseStream { status, headers,
    timings, body: BoxStream<'static, Result<Bytes, ExitError>> }` (reqwest
    `bytes_stream()`; workspace reqwest gains the `stream` feature). The plain
    `HttpRequest`/`HttpResponse` structs were deleted from proxy-protocol (agent-only,
    never serialized — verified zero users left, incl. feature-gated cfg).
  - connection.rs: request bodies stream into reqwest via `Body::wrap_stream` over a
    `poll_fn` adapter on the chunk channel (Data→Ok, End→stream end, Abort→ sets a
    shared `AtomicBool` then yields Err so reqwest tears down upstream) — no Vec
    reassembly. Response path sends `HttpResponseStart` at upstream headers (TTFB to
    the client now starts at headers), forwards chunks (`send_response_chunk` splits
    anything > BUFFER_SIZE into refcounted `Bytes::slice` sub-chunks), End on
    exhaustion, **Abort on mid-stream error** (master's `abort_http` handles
    abort-after-start: always wins → client 502 under the still-buffering master; 540
    turns this into truncation semantics). execute_http Err + abort flag → echo
    `HttpBodyAbort`; Err without flag → buffered 502 (old `send_http_response` survives
    only for this).
  - nodelay acceptance criterion verified already satisfied: every
    `connect_timed`/`connect_with_mark_timed` call site runs `configure_tunnel_socket`
    right after connect, reqwest 0.12 defaults `tcp_nodelay` on. No change.
  - Process: Opus subagent implementation (its final report was lost — it ended its
    turn waiting on a background test — but the diff was complete; audited from the
    tree) + orchestrator line-by-line audit + adversarial Opus pass. The adversarial
    pass found **1 real BLOCKER, fixed**: when an upstream responds without consuming
    the streamed request body, reqwest drops the receiver; the old send-failure
    handling evicted the sink from `http_bodies`, misrouting the rest of that id's
    frames — terminal End included — into `pending_bodies` where nothing drains them
    (ids never recur) → per-session memory leak, ~Content-Length bytes per affected
    POST. Fix: keep the dead sink registered and drop chunks on failed send; the
    End/Abort arms bound the entry's lifetime (master always sends a terminal).
    Adversarial INFOs: the request-abort machinery is dead code under today's
    buffered master (no `MasterMessage::HttpBodyAbort` emitter exists yet — 540 adds
    it); `tcp_connect_ms` now means time-to-headers (write-only metrics column, more
    accurate than the old time-to-full-body).
  - Verification (orchestrator re-run on final code): cargo check clean (forced
    recompile, zero warnings), units 20 (protocol) + 39 (master) + 12 (agent, 6 new:
    body-stream adapter Data/End/Abort + closed-channel, chunk splitting), e2e
    **24/24** with `--features quic` (run twice: pre- and post-BLOCKER-fix).

**⚠️ For 540 (two items from this session)**:
1. hyper honors a forwarded `Content-Length` header on a `wrap_stream` body
   (fixed-length framing, not chunked). The streaming master MUST deliver exactly
   Content-Length bytes into the agent or fail via the stream-error path — a short
   non-abort stream would poison upstream connections / hang. Today's buffered master
   trivially guarantees this; 540 must preserve it.
2. e2e blind spots that 540 should cover (or consciously skip): early-responding
   upstream on a POST with body (exercises the BLOCKER path above), upstream reset
   mid-response (agent Abort → master truncation), request abort mid-upload (first
   real user of the agent's abort machinery).

- **540 — master streams HTTP both directions** (session 6, 2026-06-12, proxy-mesh repo,
  commit 3bf2175 — **LOCAL ONLY until the block pushes**, completes the 541/542/540 wire
  block; PROTOCOL_VERSION still 2, proxy-agent and proxy-protocol untouched). What landed:
  - `responses.rs` rewritten around streaming: `HttpResponseStart` resolves a oneshot with
    `HttpStart { status, headers, agent_timings, body_rx: HttpBodyStream }`; the body flows
    through an **unbounded** mpsc with a `queued_bytes` cap at `MAX_HTTP_BODY_SIZE` (64MiB).
    The flow-control trilemma (no per-request wire flow control on a multiplexed link) was
    decided as: capped backlog + visible truncation — never block the shared per-agent
    dispatch loop, never unbounded memory. Cap matches the old accumulator's limit, so
    nothing that worked before is newly rejected; worst case backlog 64MiB+16KiB (agent
    chunks ≤ BUFFER_SIZE). Pre-start chunks (QUIC reordering) buffer under the same cap and
    replay in order.
  - proxy.rs: `RequestBody::{None,Buffered,Stream}`; request bodies stream into the agent
    as `HttpBodyChunk` frames concurrently with awaiting the response start (early-responding
    upstream works); client upload error emits `MasterMessage::HttpBodyAbort` — the first
    real user of 542's agent abort machinery. `MeteredHttpBody` counts bytes_down, applies
    HTTP_TIMEOUT as idle-timeout between chunks (TTFB deadline before start), records
    metrics exactly once at the terminal with real status + `response_truncated` error
    counter — never a false-clean 200. Abort-after-start = io::Error truncation on the
    client body stream, never 502. The 413 request-size cap is gone for proxied requests;
    `/api/proxy` keeps buffered input, streams output; preauth streams both directions.
  - Process: Opus subagent + line-by-line audit + adversarial pass. The audit/adversarial
    combo caught **2 BLOCKERs + 1 MAJOR in the subagent's code, all fixed by an
    orchestrator rewrite of responses.rs**: (S1) abort delivered `Err(BodyAborted)` via
    `try_send` on a bounded channel — when full (slow client), the truncation terminal was
    silently dropped → clean EOF + clean metrics on a truncated body (data corruption
    presented as success); fixed by the unbounded channel making the Err infallible (FIFO:
    Err never overtakes buffered chunks), regression test
    `abort_with_large_undrained_backlog_still_truncates`. (S2) backpressure-stall teardown
    removed the entry with no error terminal — same silent truncation; fixed structurally
    (append is sync, no timeout path exists). (M1) `append_http_body` awaited up to 30s
    inside the shared per-agent dispatch loop — one slow HTTP client head-of-line blocked
    ALL multiplexed traffic (tunnels, pongs — could trip the heartbeat reaper); fixed:
    sync append + capped backlog. A second adversarial pass on the rewrite found nothing.
  - 542's two ⚠️ items addressed: C-L exactness holds (hyper `Incoming` yields None only on
    a complete body; client error → Abort path, never a short clean stream); e2e blind spot
    covered with `spawn_early_responder` + `early_responding_upstream_on_post_with_body`
    (1MiB patterned up / 800KB patterned down, upstream answers after headers only).
  - Verification (orchestrator re-run): cargo check clean, units 20+12+47 (4 new), residue
    greps clean (no HttpAccumulator/pending_end leftovers; `collect_body_capped` only in
    proxy_api). e2e **25/25** across two runs: full run 18/25 where all 7 failures were
    proven external (httpbin.org returning 503 to a direct curl; all 7 are CONNECT-tunnel
    tests untouched by 540), https subset re-run green 8/8 after httpbin recovered.
    Ops note: the OnceLock harness leaves orphan master/agent processes that inherit the
    stdout pipe — redirect e2e output to a file and pkill orphans after.
  - Known INFO items (deliberate, bounded): `bytes_up` recorded as 0 for early-responding
    upstreams (metrics-only race); registry entry can linger after client disconnect until
    the next agent frame; the 504 arm leaves the spawned body task streaming to a dead id
    (agent pending_bodies absorbs it); tunnel try_send→spawn reorder hazard deferred to 543.

**The 541/542/540 wire block is COMPLETE** (local commits 1b2cd68, 45cda55, 3bf2175 on top
of ec23aa7). **Next**: decide the push to origin master (user gate — GitLab CI auto-deploys;
deploy masters before agents per the v2 handshake), then **543** LAST and measure-first
(premise re-verified: the mpsc hop is still there; the tunnel reorder hazard from 541's
finding 3 naturally belongs to it — bench before/after or don't merge).

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
