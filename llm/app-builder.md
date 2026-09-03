---
topic: app-builder
features: core
tokens: ~2000
requires: di-beans
---

## AppBuilder — full assembly order

### TL;DR

- `App::build` **returns** the `BootableApp`: do not call a terminal
  `serve`/`serve_auto` there — `r2e::launch!(A)` supplies `AppBuilder::new()` and
  the terminal call around your `build`.
- Builder phase, before `build_state().await`: `load_config`, `plugin`,
  `provide`/`provide_all`, `register`, `register_module(s)` — their order relative
  to each other does not matter.
- App phase, after `build_state().await`: `on_start` / `on_start_once` / `on_stop`,
  `register_controller(s)`, then `serve_auto()` (host/port from config),
  `serve(addr)` or `build()` → `Router`.
- Bean `#[consumer]` subscribers are auto-collected at `build_state()` — there is
  no explicit registration call.
- `serve()` runs startup hooks, registers consumers, starts scheduled tasks, listens
  with graceful shutdown (Ctrl-C / SIGTERM), then runs shutdown hooks.
- The HTTP drain is **bounded by default at 30s**; precedence is
  `.drain_timeout(d)` / `.drain_timeout_unbounded()` > `server.drain-timeout` > 30s,
  and no config value means "unbounded".
- `.shutdown_grace_period(d)` bounds the tracked-handle join **per handle**; an
  overflowing handle is detached, never aborted — use `track_named(name, fut)` (or
  `spawn_service::<C>()`) so the warning names it.
- `on_stop` is must-run — it executes even after a timed-out drain, so state
  reconciliation belongs there.
- Inject `r2e::rt::ShutdownToken` to stop your own work when the drain starts; it is
  a normal bean, so a `#[module]` must list it in `imports(...)`.
- `ShutdownToken` has deliberately **no `cancel()`**: cancel a `child_token()`
  instead, or override the bean with `ShutdownToken::from_token(t)` in a test.

The chain below is the body of `App::build` (return the `BootableApp` — do NOT
call a terminal `serve`/`serve_auto` there; `r2e::launch!(A)` does that). The
`AppBuilder::new()` + terminal `serve_auto()` shown here are what `launch!`
performs around your `build`.

```rust
# async fn __doc() {
AppBuilder::new()
    // ── Builder phase: every plugin, in any order relative to load_config/provide/register ──
    .load_config::<RootConfig>()                // or .load_config::<()>() for raw config only
    .plugin(Executor)                           // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)                          // scheduler runtime
    .plugin(GrpcServer::on_port("0.0.0.0:50051"))
    .plugin(Health)
    .plugin(Cors::permissive())
    .plugin(HttpTrace::new())                   // per-request span + summary line (`trace.*`)
    // subscriber: the entry point installs it; `.plugin(Tracing)` only if you opted it out
    .plugin(NormalizePath)                      // order irrelevant (Finalize stage)
    .plugin(OpenApiPlugin::new(OpenApiConfig::new("API", "1.0").with_docs_ui(true)))
    .provide(bean)                              // constructed values
    .provide_all(env)                           // every field of a #[derive(ProvideBundle)] struct
    .register::<CreatePool>()                   // beans / producers (one unified method)
    .register_module::<UserModule>()            // modules: builder phase (may bring plugins)
    .register_modules::<AppModules>()           // ...or a #[module(modules(..))] aggregate
    .build_state().await
    // ── App phase: controllers, lifecycle ──
    .on_start(|state| async move { Ok(()) })
    .on_start_once(|state| async move { Ok(()) })  // first cycle only under `r2e dev`
    .on_stop(|state| async move { if let Some(p) = state.bean::<SqlitePool>() { p.close().await; } })
    .register_controller::<UserController>()
    .register_controllers::<(AccountController, ScheduledJobs)>()  // several at once
    // bean #[consumer] subscribers auto-collected at build_state() — no explicit call
    .serve_auto().await                         // server.host/server.port from config
    // or .serve("0.0.0.0:3000").await
    // or .build() → Router
    .unwrap();
# }
```

`serve()` runs startup hooks, registers consumers, starts scheduled tasks,
listens with graceful shutdown (Ctrl-C / SIGTERM), then runs shutdown hooks.

**Shutdown budgets — which bound covers what.** Two independent options, each
covering exactly one phase; the phases run in this order:

| Phase | Bounded by | Default | On overflow |
|---|---|---|---|
| `on_drain` hooks | — | — | — |
| plugin sync + async shutdown hooks (incl. `#[pre_destroy]`) | — | — | — |
| HTTP drain (in-flight requests, listener no longer accepting) | `.drain_timeout(Duration)` or `server.drain-timeout` | **30s** (`.drain_timeout_unbounded()` opts out) | `warn!(phase = "http drain", ..)`, remaining connections abandoned, shutdown continues |
| tracked-handle join (`spawn_service`, `ServeContext::track`, gRPC/QUIC drains, `#[ws]` sessions) | `.shutdown_grace_period(Duration)`, applied **per handle** | unbounded | `warn!(phase = "tracked-handle join", service = <label>, ..)`, that handle detached (never aborted), others keep their own budget |
| `on_stop` hooks | — **always run** | — | — |

`on_stop` is **must-run**: it executes even when the drain timed out and every
tracked handle blew its grace period — that is where state reconciliation
belongs. `drain_timeout` is measured from cancellation (the moment the listener
stops accepting), not from `serve()`. It is **bounded by default (30 seconds)**
— an app that never mentions it still finishes shutting down. Precedence:
`.drain_timeout(d)` / `.drain_timeout_unbounded()` > the `server.drain-timeout`
config key (a duration: `30`, `"30s"`, `"500ms"`, `"2m"`; an unparseable
value fails the boot) > 30s. There is no
config value meaning "unbounded": dropping the bound (and accepting that one
open SSE stream can hang the process forever) is a deliberate code decision,
`.drain_timeout_unbounded()`. Under sharded serving (`server.workers`)
each worker bounds its own drain on its own child token, so both strategies
behave identically; worker runtimes are dropped only after the tracked-handle
join, so tracked work owning a socket a worker accepted (a WebSocket session)
keeps a live I/O driver for its whole grace period. The grace-period warning names the handle:
`spawn_service::<C>()` uses `C`'s type name, `ServeContext::track_named(name,
fut)` the name you give, a `#[ws(...)]` session
`ws:<Controller>::<method>`, plain `track(fut)` shows `<unnamed>`.

### `rt::ShutdownToken` — the injectable shutdown signal

Every `AppBuilder::new()` provides one, so any bean or controller can inject it
and stop its own work when the app starts draining:

```rust
use r2e::rt::ShutdownToken;

#[controller(path = "/feed")]
pub struct FeedController {
    #[inject] shutdown: ShutdownToken,
}
# fn main() {}
```

```rust,ignore
pub async fn cancelled(&self);                            // await the signal
pub fn cancelled_owned(self) -> impl Future<Output = ()> + Send + 'static;
pub fn is_cancelled(&self) -> bool;
pub fn child_token(&self) -> CancelToken;                 // your own sub-scope
pub fn from_token(token: CancelToken) -> Self;            // for `.override_bean(..)` in tests
```

- It is a **child of the app shutdown token**, cancelled when the drain starts —
  and by a drop guard on the uncontrolled exits (a panic, an aborted boot, a
  dropped `run()` future).
- There is deliberately **no `cancel()`**: user code must not be able to shut the
  application down by cancelling a bean it happens to hold. Cancel a
  `child_token()` instead — that stops your scope and nothing else. A test that
  needs to drive the signal by hand overrides the bean:
  `.override_bean(ShutdownToken::from_token(my_token))`.
- It is **a normal bean, not an ambient one**: a `#[module]` still has to list it
  in `imports(...)` like any other dependency.
- Under `r2e dev` it is **cycle-scoped**: each hot patch gets a fresh token (the
  previous cycle's is already cancelled) and every bean that captured a clone is
  rebuilt with it, so nothing is left holding a dead token.
