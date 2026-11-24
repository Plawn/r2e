# r2e-rt

Async-runtime facade for R2E — the single crate in the workspace that names `tokio`.

## Overview

`r2e-rt` centralises every direct `tokio::*` touchpoint so that a runtime change (or the thread-per-core sharded serving mode) is made in one place. It sits at the bottom of the dependency graph and depends on no other R2E crate. `r2e-core` re-exports it as `r2e_core::rt`, which is what `r2e::rt` resolves to.

Types that appear in R2E's public API are **wrapped** (newtypes with the tokio type private: `JobHandle`, `JoinError`, `Elapsed`, `CancelToken`), so an app can use R2E's shutdown API without adding `tokio-util` itself. Runtime-neutral primitives (the `sync` module, `select!`/`pin!`/`join!`/`task_local!`, `Instant`, `Interval`, `JoinSet`, `stream`) are **re-exported** as-is.

## Usage

Applications do not depend on this crate directly — use it through the facade:

```rust
use r2e::rt;

let handle = rt::spawn_ctl(async { /* background work on the control plane */ });
rt::sleep(std::time::Duration::from_millis(10)).await;
```

Crates below `r2e-core` (and framework crates that cannot depend on it) use it directly:

```toml
[dependencies]
r2e-rt = "0.6"
```

## What is in scope

| Area | Items |
|------|-------|
| Task spawning | `spawn`, `spawn_blocking`, `spawn_ctl` → `JobHandle<T>`, `yield_now`, `in_runtime` |
| Control plane | `set_control_plane`, `current_handle` |
| Time | `sleep`, `sleep_until`, `timeout`, `interval`, `Instant` |
| Network | `bind_tcp`, `lookup_host`, `TcpListener`, `TcpStream` |
| Async I/O | the `io` module (`AsyncRead`/`AsyncWrite` + extension traits) |
| Signals / cancellation | `shutdown_signal`, `CancelToken`, `CancelDropGuard`, `ShutdownToken` (injectable app-shutdown bean) |
| Synchronisation | the `sync` module (`mpsc`, `oneshot`, `broadcast`, `watch`, `Mutex`, `RwLock`, `Notify`, `Semaphore`, `OnceCell`) |
| Control flow | `select!`, `pin!`, `join!`, `JoinSet`, `task_local!`, `stream` |
| Runtime construction | `RuntimeBuilder`, `Runtime`, `RuntimeHandle`, `RuntimeId`, `block_on`, `block_in_place` |

## Control plane and data plane

With `server.workers` set, HTTP requests run on N `current_thread` worker runtimes (the data plane), while scheduler tasks, services, event consumers, executor jobs and lazy beans must run on the main multi-thread runtime (the control plane). `spawn_ctl` routes a future onto the control plane when called from a worker thread, and behaves like `spawn` otherwise. See [`docs/features/19-sharded-serving.md`](../docs/features/19-sharded-serving.md).

## Features

| Feature | Description |
|---------|-------------|
| `test-util` | Paused-clock runtimes (`#[r2e::test(start_paused = true)]`); enabled through the facade's `test-util` feature |

## License

Apache-2.0
