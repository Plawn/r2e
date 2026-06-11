# Sharded Serving (SO_REUSEPORT)

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

The lifecycle (consumer registration, serve/startup hooks, QUIC spawn, shutdown
phase, grace period) is **shared** with the single-listener path — only the
"bind + serve" middle section differs (`PreparedApp::run_inner` + the internal
`ServeStrategy` enum).

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
- **QUIC/HTTP3 is out of scope.** In sharded mode the QUIC endpoint (if
  configured) stays on the main runtime exactly as today; sharding affects TCP
  only.
- **A worker dying mid-run does not stop the app.** If one worker exits early
  (serve error or panic), the remaining workers keep serving — capacity is
  degraded by 1/N and nothing restarts the dead worker. The failure is logged
  immediately, but a worker serve `Err` only propagates as the overall run
  error after shutdown, when all workers are joined. (The single-listener path,
  by contrast, tears the whole server down on a serve error.)
- **Lazy beans (NOTE 536 → 537).** A worker runs a `current_thread` runtime, so
  a lazy bean first *resolved from within a worker* requires the
  `lazy-fallback-runtime` feature (`block_in_place` panics on current-thread
  runtimes). In practice lazy beans are resolved during state construction on
  the main multi-thread runtime, so this only bites if a lazy bean is first
  touched from a worker. The proper fix (a dedicated control-plane runtime) is
  deferred to task 537.
