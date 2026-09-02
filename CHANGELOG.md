# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

> **Tags vs versions.** Since tag `v0.3.0`, git tags follow the workspace
> version: `vX.Y.*` is the compatibility series declared in the root
> `Cargo.toml`, and the patch is a monotone release counter within that series
> (a breaking change bumps `X.Y` in `Cargo.toml`, which starts a new tag
> series). Earlier tags were a pure release counter detached from the manifest:
> tags `v0.2.132`–`v0.2.163` actually contain workspace version `0.3.0`, and
> the 0.3 plugin-API rework ships from **`v0.2.140`** onward (see
> [`docs/migration/plugin-api.md`](docs/migration/plugin-api.md)).

## [Unreleased]

### Added

- **Per-request span enrichment channel** (task #1015): the `HttpTrace` layer
  now publishes the request span as the `RequestSpan` request extension —
  handlers take it as a parameter and `record(..)` domain fields their
  `MakeRequestSpan` declared `Empty` (`session_id`, `tenant_id`, …), at any
  call depth and without task-local plumbing (excluded routes yield a no-op
  `Span::none()`). A new defaulted `MakeRequestSpan::make_state` allocates an
  optional per-request `SpanState` slot (type-erased `Arc`), published as a
  request extension and handed back to `on_response` — the way values written
  *during* the request reach a custom summary event, since span fields are
  write-only. **Breaking**: `MakeRequestSpan::on_response` gains a
  `state: Option<&SpanState>` parameter (default impl unchanged otherwise).

- **`TestApp` can reuse an `App::Env` across boots** (task #988): three new
  boots skip `A::setup()` and build on an environment the caller already owns —
  `TestApp::boot_env::<A>(env)`, `TestApp::boot_with_env::<A>(env, configure)`,
  `TestApp::boot_plain_env::<A>(env, configure)`, plus `try_boot_env` /
  `try_boot_with_env` / `try_boot_plain_env`. `App::Env` is already
  `Clone + Send + Sync + 'static`, so a test binary builds the expensive part
  once and boots every test off it instead of replaying pools and migrations
  per test (`#[before_all]` only amortises inside one suite).

  ```rust
  use r2e_test::SharedEnv;

  static ENV: SharedEnv<MyApp> = SharedEnv::new();

  #[r2e::test(app = my_app::MyApp, env = ENV.get().await)]
  async fn lists_users(app: TestApp) {
      app.get("/users").send().await.assert_ok();
  }
  ```

  `r2e_test::SharedEnv<A>` is the supported way to memoise it: `new()` /
  `with(init)` are `const` (so they go into a `static`), `get().await` /
  `try_get().await` build the environment **once per process on a runtime
  `r2e-test` owns and never shuts down**, and concurrent first callers share the
  one run. A bare `OnceCell`/`LazyLock` must not be used: `#[r2e::test]` builds
  one runtime per test and drops it at the end of the test, so an environment
  initialised there keeps its value but loses its reactor (listeners, pool
  keep-alive tasks, timers, anything `setup` spawned), and later tests hang on
  an inert environment.

  `#[r2e::test]` and `#[r2e::test_suite]` gained the matching `env = <expr>`
  knob (evaluated inside the test's async block, composes with `with = …` and
  `jwt = false`, requires `app = …`; on a suite it also requires a
  `#[before_all]` that binds the booted app, since that hook is what evaluates
  it — likewise for `with = …` / `jwt = …`). Everything else is unchanged:
  `test` profile, pinned `TestJwt` validators, the production startup phase,
  `shutdown()`. **Isolation is the caller's job** — a shared `Env` is shared
  state across concurrently running tests. The harness never disposes the `Env`
  itself, but `shutdown()` does run whatever `A::build` registered, so an app
  that hands an `Env`-owned resource to a disposer still invalidates it for
  later boots.

- **Feature modules own their gRPC services** (task #989): `#[module]` gained a
  `grpc_services(...)` key, the transport peer of `controllers(...)`. A vertical
  slice now declares its gRPC service next to its HTTP controllers, and that
  service may inject the module's **private** providers — which the app-level
  `.register_grpc_service::<S>()` cannot, since it checks the service's deps
  against the application state (so a module bean had to be exported to be
  injectable).

  ```rust
  #[module(providers(GreetingRepo), grpc_services(GreeterService))]  // GreetingRepo stays private
  pub struct GreetingModule;

  AppBuilder::new()
      .plugin(GrpcServer::on_port("0.0.0.0:50051"))
      .register_module::<GreetingModule>()   // no .register_grpc_service::<_>() needed
      .build_state()
      .await
  ```

  The services are dependency-checked **module-locally** at `register_module`
  (deps ⊆ providers ∪ imports, `#[intercept(...)]` spec deps folded in, exactly
  like `M::Controllers`) and registered by `build_state()` from the module's
  retained `BeanContext`, in declaration order after the module's controllers.
  The key also implies the `GrpcServer` plugin: the macro appends it to the
  module's `RequiredPlugins`, so forgetting `.plugin(GrpcServer::...)` is a
  compile error **naming `GrpcServer`** rather than a service silently
  registered into a registry nobody drains. A module may equally bring the
  plugin itself with `plugins(GrpcServer = GrpcServer::on_port(..))`.
  `RequiredPlugins` is verified by checking the plugin's *provisions* against
  the provision list, so `GrpcMarker` — what `GrpcServer` provides — is now
  unconstructible outside r2e-grpc: nobody can hand-`.provide(..)` it to make a
  module compile without the plugin (and hence without the registry the module
  registers into). A hand-written `impl FeatureModule` that skips
  `RequiredPlugins` altogether still fails at boot, with
  `BeanError::MissingTransportPlugin` naming both the plugin and the module.

  Each service is registered **once**: a name already in the registry — the same
  service in two modules' `grpc_services(..)`, or in a module *and* an app-level
  `.register_grpc_service::<S>()` — fails instead of handing tonic two
  overlapping route sets under one name. The module path reports
  `BeanError::DuplicateEndpoint` on the `try_build_state()` channel; the
  app-level call panics (that call site already panics for a missing plugin);
  a service listed twice in one `grpc_services(..)` is a macro compile error.

  r2e-core stays transport-agnostic: `FeatureModule` gained
  `type Endpoints: ModuleEndpointSet` (type-level `Deps` only) and the
  value-level `ModuleEndpoints<T>` registration hook, both implemented in
  r2e-grpc for the new `r2e_grpc::ModuleGrpcServices<(A, B)>` (what the macro
  generates). Modules without `grpc_services(..)` emit `type Endpoints = ();`
  and no r2e-grpc path, so they still compile in apps without the gRPC feature.

  **BREAKING (pre-production, no compatibility shim)**:
  - hand-written `impl FeatureModule` blocks must add `type Endpoints = ();`
    (stable Rust has no associated-type defaults) — the `#[module]` macro
    generates it;
  - `RegisterModule` gained a witness type parameter (`EndpIdx`) — only visible
    to code that names the trait's parameters explicitly;
  - three new `BeanError` variants, `EndpointConfig`, `MissingTransportPlugin`
    and `DuplicateEndpoint` — an exhaustive `match` on `BeanError` must add arms;
  - `GrpcServiceRegistry::add_service` returns `Result<(), DuplicateService>`
    (it was `()`), and `GrpcMarker` is no longer constructible outside
    r2e-grpc;
  - registering the same gRPC service twice now fails (module: boot error;
    app-level `.register_grpc_service::<S>()`: panic) instead of silently
    double-registering it.

- **`rt::RuntimeId`** (`Runtime::id()` / `RuntimeHandle::id()`): the identity of
  a runtime, comparable across threads, for asserting that two pieces of work
  share one reactor. Used by `#[r2e::test_suite]`'s guard-rail (see Fixed).
  Ids are unique only among *live* runtimes: once a runtime is dropped its id
  may be reused by a later, unrelated one, so an id only proves shared identity
  when compared against a runtime known to be alive (which is how the suite
  guard uses it — the suite owns the runtime it names).

- **`rt::Runtime::shutdown_timeout` / `shutdown_background`**: shut a runtime
  down explicitly, for runtimes parked in a `static` that can never be dropped
  by going out of scope. `#[r2e::test_suite]` uses the first one at teardown.

- **Worker scopes and verifiable multi-worker serving** (task #990, ADR
  `docs/adr/0001-worker-scopes-and-planes.md`): `WorkerInfo` (stable worker
  identity — id / count / role / effective CPU — readable anywhere, incl. as a
  handler parameter), `WorkerLocal<T>` + `AppBuilder::worker_local` (exactly one
  `!Send`-capable `T` per worker, built/used/dropped on its worker thread),
  `WorkerSet` + `WorkerState` + `WorkerHealth` (aggregated per-worker lifecycle
  and errors, health indicator), `Mailboxes<M>` (counted cross-worker messaging
  with `send_to`/`broadcast`/`ask_all`), `r2e::runtime::ingress`
  (`reuseport_tcp`/`reuseport_udp`/`adopt_*`, `AffinityError::Unsupported` — no
  silent fallback), `WorkerCollector` in `r2e-prometheus` (`r2e_worker_*`
  series), and `WorkerHarness` for deterministic tests. Docs in
  `docs/features/19-sharded-serving.md`; example `examples/example-worker-udp`
  rewritten as a shared-nothing service with control-plane aggregation.

- **grpc-web on the multiplexed gRPC transport** (feature `grpc-web`, `web` on
  `r2e-grpc`): `GrpcServer::multiplexed().with_grpc_web()` (or
  `.with_grpc_web_cors(CorsLayer)`) adds a `tonic-web` arm to
  `MultiplexService` for `application/grpc-web`, `grpc-web+proto` and
  `grpc-web-text` requests over HTTP/1.1 and HTTP/2, with CORS preflight
  handling. Without it grpc-web requests still get `415` + a boot warning.

- **`r2e::http::IntoHttpResponse`** — R2E's own response-conversion contract,
  the counterpart of `FromRequestPartsVia` on the extract side. R2E error types
  (`HttpError`, `ParamError`, `MultipartError`, `RequestId`, `SecurityError`,
  `TenantError`, `OidcError`) and everything `#[derive(ApiError)]` generates now
  implement **this** trait instead of the HTTP backend's `IntoResponse`, and
  bridge to the backend through a single macro:

  ```rust
  impl IntoHttpResponse for MyError {
      fn into_http_response(self) -> Response { /* … */ }
  }
  r2e::http::impl_into_response!(MyError);
  ```

  **Not a break**: the bridge emits the backend impl, so every type that was
  returnable from a handler still is, and `Result<T, E>` / `(StatusCode, T)`
  composition is unchanged. A hand-written `impl IntoResponse for MyError` also
  keeps working — `IntoHttpResponse` is the recommended way, not the only one.
  The macro is a macro rather than a blanket impl because
  `impl<T: IntoHttpResponse> IntoResponse for T` is an orphan impl and the
  mirror blanket would forbid all per-type impls; see `r2e-http/src/response.rs`.
  `IntoHttpResponse` is in the prelude.

- **`r2e::http::axum_compat`** — the explicit escape hatch to the raw `axum`
  API (`use r2e::http::axum_compat::axum;`), for the cases a re-export shim
  cannot cover: tower layers with axum-typed bounds, `axum::debug_handler`,
  third-party crates whose API is spelled in axum types. This settles §5.3d of
  `plans/runtime-http-dependency-containment.md` as **decision A**: R2E's public
  promise is *R2E types* under `r2e::http` / `r2e::prelude` plus R2E's own
  contracts (`IntoHttpResponse`, `FromRequestPartsVia`); axum stays reachable,
  but only through a name you have to type on purpose. Apps should still not
  add `axum` to their own `Cargo.toml`.

- **New crate `r2e-rt`** — the async-runtime facade, sitting at the **bottom**
  of the workspace dependency graph (below `r2e-http`). It is now the single
  workspace member allowed to name `tokio` / `tokio-util` / `tokio-stream`
  directly, so swapping the runtime — or moving further towards thread-per-core
  sharded runtimes — is a change in one crate instead of a hunt across dozens of
  call sites. Two enforcement scripts freeze the boundary
  (`scripts/check-dep-boundary.sh`, `scripts/check-source-boundary.sh`).
  `r2e-core/src/rt.rs` moved into it wholesale and `r2e_core::rt` is now a
  re-export, so **`r2e::rt::…` / `r2e_core::rt::…` keep resolving to exactly
  what they always did** (`spawn`, `spawn_ctl`, `spawn_blocking`, `JobHandle`,
  `sleep`, `timeout`, `interval`, `bind_tcp`, `shutdown_signal`, …).
  New in the facade, on top of the moved surface:
  - `rt::CancelToken` / `rt::CancelDropGuard` — wrappers over
    `tokio_util::sync::CancellationToken` / `DropGuard`, so an app can consume
    R2E's shutdown API without adding `tokio-util` to its own `Cargo.toml`.
    `From` conversions both ways keep the not-yet-migrated crates working.
  - `rt::sync` — re-exports of `mpsc`, `oneshot`, `broadcast`, `watch`,
    `Mutex`, `RwLock`, `Notify`, `Semaphore`, `OnceCell`.
  - `rt::{select!, pin!, join!}`, `rt::JoinSet`, `rt::stream`,
    `rt::{RuntimeBuilder, Runtime, block_on}`.
  - `rt::Instant` + `rt::sleep_until(deadline)` — the deadline form of
    `rt::sleep`, on the runtime's own monotonic clock; what a timer wheel driven
    by absolute fire times needs (the scheduler's min-heap driver).
  - `rt::yield_now()` and `rt::in_runtime()` — the latter is the non-panicking
    probe behind `current_handle`, for synchronous paths that may run outside a
    runtime (a `Drop` impl detaching cleanup work).
  - A non-default `test-util` feature (`tokio/test-util`), off by default
    because paused clocks must not reach the whole workspace through feature
    unification.
  - `rt::TcpStream` and the `rt::io` module (`AsyncRead` / `AsyncWrite` and
    their `…Ext` traits, `BufReader`, `BufWriter`, `duplex`) — re-exports, the
    same treatment as `rt::TcpListener` and `rt::sync`. They are what raw-socket
    test code and byte-stream plumbing need, and their absence was the last
    reason to keep a direct `tokio` dependency around. `rt::stream::wrappers`
    also carries `TcpListenerStream` now (tokio-stream's `net` feature).

### Fixed

- **`r2e::prelude` no longer ambiguous with both data backends enabled**
  (task #1016). The prelude glob-re-exported `r2e_data_sqlx::prelude::*` and
  `r2e_data_diesel::prelude::*` unconditionally, and the two backends
  deliberately export mirrored names (`DbPool`, `DbTx`, `Tx`,
  `DataSourceHealth`, `TenantPools`, `TenantTx`) — so any build enabling both
  `data-sqlx` and `data-diesel` (a dual-backend app, or a workspace sibling
  pulling the other backend through cargo feature unification) made every use
  of a mirrored name a deny-by-default `ambiguous_glob_imports` error. With
  exactly one backend enabled its prelude joins `r2e::prelude` as before; with
  both, only the backend-unique names (`SqlxDataSource`, `SqlxTx`,
  `DieselDataSource`, `DieselTx`) remain and the mirrored ones are imported
  explicitly from `r2e::r2e_data_sqlx` / `r2e::r2e_data_diesel` (an explicit
  import shadows the glob, so it is stable under both modes).

- **`#[sse]` / `#[ws]` routes publish their parameters in the OpenAPI spec**
  (task #1013, follow-up of #1009). Streaming metadata hardcoded
  `params: vec![]`, so a `#[derive(Params)]` argument — which the generated
  handler really does extract from the request — never appeared in
  `/openapi.json`, and neither did a `Path<T>` argument. Both route kinds now
  build their `RouteInfo.params` through the same code path as a verb route
  (`Path(name): Path<T>` literals + the `ParamsMetadata` autoref probe, then
  deduplicated), so moving a documented `#[get]` to `#[sse]` keeps its
  parameters as well as its prose. A WebSocket method's `WsStream`/`WebSocket`
  argument comes from the upgrade rather than an extractor and is excluded.

- **`r2e-core`'s `runtime` test target is green under `--features dev-reload`
  again** (task #995). Two independent causes, neither of which CI saw (no
  workflow runs the `dev-reload` feature). (1) The builder-level per-worker
  service test served a sharded app, but `dev-reload` deliberately forces the
  single cached-listener path, so `run()` rejects the registration by design;
  the test is now compiled out under the feature and replaced by one that
  asserts the rejection. (2) The dev-reload hot-patch tests shared a process
  with the ordinary serving tests. `mark_hot_reload_loop()` is process-global
  and one-way, so once a dev test had armed it the next served app set
  `LIFECYCLE_INITIALIZED` — after which *every* later `run()` in the binary
  skipped consumers, serve hooks and startup hooks and quietly lost its
  `spawn_service` tasks (`shutdown_budget::grace_period_bounds_a_stubborn_service_and_names_it`
  was the visible casualty). No lock can fix that across parallel test threads,
  so the dev-reload tests now live in their own target,
  `r2e-core/tests/dev_reload/`, and the `runtime` target no longer needs the
  `dev_serial` lock at all.

- **The `dev-reload` per-worker-service error no longer gives impossible
  advice.** It used to be built from `PER_WORKER_REQUIRES_SHARDING_MSG`, so it
  told you to set `server.workers` — a key `dev-reload` ignores. It now states
  that the feature forces single-listener serving and that per-worker services
  require a build without `dev-reload` (and a platform with SO_REUSEPORT
  sharding — dropping the feature is necessary, not sufficient).

- **Attribute macros no longer drop the attributes you write** (task #985).
  Several attribute macros rebuild the item they annotate from its pieces
  (visibility + signature parts + body) so they can strip R2E's own parameter
  and field attributes. Those rebuilds silently discarded everything else.
  - `#[producer]` dropped the whole `attrs` list of the annotated function:
    `#[allow]`/`#[deny]`, `#[inline]`, `#[deprecated]`, `#[must_use]` and doc
    comments written on a producer did nothing. It also dropped `const` and
    `extern "…"` from the signature. All of them are forwarded now, and the
    generated bean struct carries a doc comment of its own so
    `#![deny(missing_docs)]` crates keep building. `#[deprecated]` warns at a
    direct call to the function; the generated struct is a separate item and is
    not itself deprecated, so `.register::<CreatePool>()` stays quiet.
  - `#[routes]` dropped the attributes on the `impl` block (a `#[allow(...)]`
    or doc comment above `impl MyController` vanished) and dropped every
    associated item that was neither a route, a `#[consumer]`, a `#[scheduled]`
    nor a lifecycle hook — an associated `const`, an associated `type` or a
    plain helper `fn` written in a `#[routes]` block disappeared from the
    build. Impl attributes now reach both synthesized impls, and the other
    items stay on the controller core. Note that a route body's `Self` is the
    request façade, so reach an associated const through the controller name
    (`MyController::PAGE_SIZE`). Because there are *two* synthesized impls,
    only **inert** attributes may sit below `#[routes]` — doc comments,
    `#[allow]`/`#[warn]`/`#[deny]`/`#[expect]`/`#[forbid]`, `#[deprecated]`,
    `#[cfg]`, `#[cfg_attr]` and tool attributes (`#[rustfmt::skip]`). Anything
    else (an attribute macro) would expand once per impl, so it is a compile
    error pointing at the position where it runs exactly once: above
    `#[routes]`.
  - `#[bean]` dropped the attributes and the `const`/`extern` pieces of the
    constructor it re-emits.
  - `#[async_exec]` dropped parameter attributes (`#[cfg]` on a parameter,
    `#[allow]`) when re-emitting the wrapper's parameter list. A parameter
    `#[cfg]` is now forwarded to the *forwarding call* as well, so a gated-out
    parameter disappears from the signature and the call together instead of
    leaving the disabled build with an unbound argument.
  - `#[controller]` projects a request-scoped field's attributes onto the
    generated request extractor and façade, and the generated code that binds
    them carries `#[allow(deprecated, non_snake_case)]`: a `#[deprecated]`
    request field warns where *you* read it, not from inside framework code, so
    a crate under `#![deny(deprecated)]` still builds.

  Because rustc evaluates an item-level `#[cfg]` (and a `#[cfg_attr]` expanding
  to one) *before* it invokes an attribute macro — in either attribute order —
  a `#[cfg]`'d-out producer, controller, `#[routes]` impl or bean never reaches
  the macro at all, and no generated impl is left dangling. That is pinned by
  tests rather than assumed (`r2e-core/tests/di/producer_attrs.rs` and
  `r2e-core/tests/controller/attrs.rs`).

  One signature piece is **rejected** rather than forwarded (breaking, but no
  such code compiled before either): an `unsafe fn` `#[producer]` or `#[bean]`
  constructor. R2E generates a *safe* `Producer::produce` / `Bean::build` that
  is the only caller, and the bean graph cannot discharge an `unsafe` contract
  it knows nothing about — re-emitting the signature verbatim is an E0133, and
  adding an `unsafe { }` block around the generated call would sign the
  contract on the user's behalf. Drop `unsafe` from the signature and keep the
  `unsafe { }` block, with its SAFETY comment, inside the body.

- **`#[producer]` now emits `#[allow(clippy::too_many_arguments)]`** on the
  function and on the generated `Producer` impl. A producer takes one parameter
  per dependency, so clippy's 7-argument threshold fires on perfectly
  idiomatic producers and (before the fix above) could not even be silenced.
  User attributes are emitted after it, so `#[warn(clippy::too_many_arguments)]`
  on the function opts back in.

- **`#[r2e::test_suite]` now builds ONE runtime per suite, not one per `#[case]`**
  (task #986). The suite value lives in a module-level `OnceLock` that outlives
  every case, but each generated `#[test]` used to build — and then drop — its
  own runtime. Anything `#[before_all]` amortised that is bound to a reactor (a
  `TestApp`, a `sqlx` pool, a socket, a spawned task, a timer) went inert after
  case 1; because such a resource stops waking rather than erroring, the suite
  failed far from the cause, typically as `PoolTimedOut`. The runtime is now
  owned by `SuiteCell` in that same `OnceLock` and is never dropped, so
  `#[before_all]`, `#[before_each]`, every case, `#[after_each]` and
  `#[after_all]` share one reactor. `#[case(order = N)]` and the per-case libtest
  `#[test]` are unchanged; the runtime knobs (`flavor`, `worker_threads`,
  `start_paused`, …) stay on `#[r2e::test_suite(...)]` and now configure that
  single runtime — note `start_paused` means one paused clock for the whole
  suite instead of a fresh one per case. Guard-rail: every phase
  (`#[before_all]`, each case, `#[after_each]`, `#[after_all]`) asserts from
  inside its `block_on` that it is on the suite runtime and panics naming both
  runtimes if not.

  Teardown: the last case to finish runs `#[after_all]`, then drops the suite
  value *inside* the runtime (so a socket or pool still has its driver in
  `Drop`) and shuts the runtime down with a one-second grace for blocking work.
  Without that the suite's worker threads and detached tasks would outlive it
  for the rest of the test process, since the `OnceLock` is never dropped.
  Anything reaching the suite after teardown panics by name instead of hanging.

  "Last case" is counted against the number of generated `#[case]`s, because
  libtest does not expose which tests the process actually selected. So a
  filtered run (`cargo test some_case`) runs `#[before_all]` and the case but
  never `#[after_all]` — the suite value is leaked to process exit, as before.
  For the same reason `#[ignore]` on a `#[case]` is now a **compile error**:
  it would either suppress teardown entirely or let teardown fire before the
  ignored case runs. Skip inside the case body instead.

  Runtime knobs that make the builder *panic* rather than return an error
  (`start_paused` without `flavor = "current_thread"`, a zero `worker_threads`
  / `max_blocking_threads` / `global_queue_interval` / `event_interval`, a blank
  `thread_name`) are now rejected at macro expansion with a spanned compile
  error, on `#[r2e::main]` / `#[r2e::test]` / `#[r2e::test_suite]` alike; any
  remaining builder panic is caught and re-raised naming the suite or test that
  asked for it.

### Changed

- **Release tags now follow the workspace version** (task #1011). The release
  workflow no longer bumps the patch of whatever the latest tag was: it reads
  `version` from the root `Cargo.toml`, tags in that `vX.Y.*` series (patch =
  release counter), and refuses member crates that don't use
  `version.workspace = true`. First aligned tag: `v0.3.0`. The tag ↔ version
  correspondence for the pre-alignment series is documented at the top of this
  file.

- **Perf: one `AuthenticatedUser` per authenticated MCP request** (task #993).
  The auth layer deposited the caller twice — standalone for the identity
  extractor, and inside `McpPrincipal` — so every request deep-copied the
  claims tree, flattened `extra` map included, and the cost grew with whatever
  the IdP put in the token. **Breaking:** `McpPrincipal.user` is now an
  `Arc<AuthenticatedUser>`. Reads are unchanged (`principal.user.sub` goes
  through `Deref`); an owned copy is `(*principal.user).clone()`. The layer
  deposits `Arc::clone(&principal.user)` as the identity extension, and the
  `#[tool]`/`#[resource]`/`#[prompt]` codegen now resolves an identity
  parameter through the new `ToolCall::identity::<T>()` (also on `ResourceCall`
  / `PromptCall`), which looks for `Arc<T>` first and materializes the owned
  `T` only for a member that actually declares one — a member taking
  `ToolCall` can read `call.extension::<Arc<AuthenticatedUser>>()` and copy
  nothing. An authenticated `tools/call` with a 32 KiB claims tree went from
  764 allocations / 129,176 B per request to **200 / 32,976 B** — the same
  cost as a caller with no extra claims at all — and `McpPrincipal::clone`
  (layer, opaque-token cache, `check_access`) from 282 allocations / 47,300 B
  to **0**. Guarded by `r2e-mcp/tests/hotpath/principal.rs` and
  `r2e-mcp/tests/auth/identity.rs`; numbers in
  `docs/claude/hot-path-clone-audit.md`.

- **Perf: MCP `tools/list` no longer re-allocates the tool metadata**
  (task #994). Every `tools/list` clones the wire payload built at boot (and
  every `tools/call` clones one element of it), but rmcp's `Tool` stores
  `name`/`description` as `Cow<'static, str>` while `ToolRoute::description`
  was an `Option<String>` — so each clone re-allocated a string `#[tool]` had
  emitted as a literal. `ToolRoute::description` is now
  `Option<Cow<'static, str>>` and the macro emits `Cow::Borrowed`: a six-tool
  `tools/list` clone went from 7 allocations / 1080 B to **1 allocation /
  1056 B** (the destination `Vec` alone), and stays there with long
  descriptions where the old path cost 2748 B. **Breaking** for hand-built
  `ToolRoute`s only: `description: Some(s.into())` where `s: String`,
  `Some("…".into())` for a literal — the macro path and the wire format are
  unchanged (pinned by `r2e-mcp/tests/server/wire_golden.rs`). `Resource`,
  `ResourceTemplate` and `Prompt` are `String`-typed in rmcp itself, so their
  lists still copy their strings; both R2E-side halves (built once at boot,
  visibility filter applied before cloning) were already in place. Guarded by
  `r2e-mcp/tests/hotpath/lists.rs`; numbers in
  `docs/claude/hot-path-clone-audit.md`.

- **Perf: the router state is one `Arc`** (task #992). The HTTP backend clones
  the router state on *every* request, whether or not a handler asks for it, so
  installing the resolved bean HList directly meant one bean `Clone` per bean
  per request — O(N) in the width of the graph, and a deep copy for any bean
  that owns its data. `build_state()` now wraps the materialized list in the new
  `r2e::BeanState<L>` (the list behind a single `Arc`), so the per-request state
  clone costs O(1) at any graph size — the backend takes two state clones per
  request, and each is now a refcount bump rather than one clone per bean:
  measured on a 64-bean state, 128 bean clones per request → 0, and for beans
  owning a `String`, 142 allocations / 10389 B per request → 14 / 1053 B (the
  same as at 8 beans). `state.get::<T>()`, `state.bean::<T>()`, `HasBean` index
  witnesses, `Contains`/`AllSatisfied` bounds and `FromRequestPartsVia` are all
  forwarded through the wrapper, each keeping the cost it already had —
  `get` a fixed-offset field read (now behind one dereference), `bean` the
  same runtime `TypeId` walk it always was — and no application code,
  controller, plugin or macro changes. **Mildly breaking**:
  the state type is now `BeanState<HCons<…>>`, so code that spells the state
  type out (a hand-written `AppBuilder<HCons<A, HNil>>` annotation, a
  hand-assembled test state) must wrap it — `BeanState::new(list)`. Guarded by
  `examples/example-app/tests/hotpath/state.rs`; numbers in
  `docs/claude/hot-path-clone-audit.md`.

- **BREAKING (`r2e-macros`)**: `#[cfg]` / `#[cfg_attr]` on a **request-scoped**
  controller field (`#[inject(identity)]` / `#[inject(request)]`) is now a
  compile error instead of a silent no-op (task #985). Those fields are
  projected into a positional marker tuple on the generated request extractor,
  which cannot be gated element-wise; conditionally compiling one used to
  produce a mismatched extractor rather than the field the author asked for.
  `#[cfg]` the whole controller instead. App-scoped `#[inject]` / `#[config]`
  fields are unaffected.

- **BREAKING (`r2e-macros`)**: a plain `#[r2e::test]` with parameters is now a
  compile error naming `#[r2e::test(app = MyApp)]` (task #985). Parameters are
  bound from the booted `TestApp`; without an `app = …` there is nothing to
  bind them from, and the generated `#[test]` fn used to fail with a confusing
  libtest signature error.

- **Perf (no API change)**: constant error bodies are no longer built through
  `serde_json::json!` on every response. `SecurityError` (401/503), the panic
  handler's 500, and the rate limiter's 429 / 401 now return a pre-serialized
  `&'static str` body via the new `r2e::http::response::static_json(status,
  body)` helper — `Bytes::from_static`, so no `Value` map allocation and no
  serializer pass per rejection. This is the hot path under unauthenticated or
  throttled traffic. Response bodies are byte-identical. Dynamic messages
  (`ParamError`, `MultipartError`, `HttpError::from_status`) keep going through
  `Json`/`json!`, which escapes interpolated values correctly.

- **BREAKING (`r2e-core`)**: the shutdown-token surface now hands out
  `r2e::rt::CancelToken` instead of `tokio_util::sync::CancellationToken` —
  `ServeContext::shutdown_token()`, `ConfigWatchContext::{new, shutdown_token}`
  and `LiveConfigReceiver::drive`. Call sites that only `select!` on the token
  or pass it along are unaffected; a site that needs the raw tokio-util token
  (tonic's `cancelled_owned()`, say) converts with `.into()` / `.into_inner()`.

- **BREAKING (`r2e-events`, `r2e-scheduler`)**: the same flip reaches the
  event-bus and scheduler surfaces, which now speak `r2e::rt::CancelToken`:
  `BackendState::{poller_cancels, register_poller_cancel}` and
  `reconnect_loop(…, cancel: &CancelToken, …)` in `r2e-events`;
  `SchedulerHandle::{new, channel, token}`, `jobs_driver`, `start_jobs` and the
  `CancelToken` **bean** the `Scheduler` plugin provides (an app injecting the
  scheduler token writes `#[inject] cancel: CancelToken` now) in
  `r2e-scheduler`. `From` converts both ways with
  `tokio_util::sync::CancellationToken`, so a call site that needs the raw token
  adds `.into()`.

- **BREAKING (`r2e-core`)**: `ServiceComponent::start` now takes
  `r2e::rt::CancelToken` instead of `tokio_util::sync::CancellationToken`.
  Hand-written background services update their signature (`async fn start(self,
  shutdown: CancelToken)`); `#[derive(BackgroundService)]` users update the
  `run` method it delegates to (`async fn run(&self, shutdown: CancelToken)`).
  With that flip `r2e-tenant`, `r2e-data-sqlx` and `r2e-data-diesel` dropped
  their last `tokio-util` dependency.

- **`r2e-core` no longer depends on `tokio` / `tokio-util` / `tokio-stream` at
  all** (dev-dependencies aside): every internal call site — the builder and
  prepared-server paths, sharded serving, lazy-bean resolution, live-config
  watching, health, SSE/WS, dev-reload — goes through `r2e_core::rt`. Sharded
  serving in particular is now expressible on the facade thanks to two
  additions: `rt::RuntimeHandle` (a wrapper over `tokio::runtime::Handle`, now
  the type of `rt::current_handle`, `rt::control_plane_handle`,
  `rt::set_control_plane` and `Runtime::handle`) and `rt::TcpListener`
  (re-exported, since axum's `serve` takes the concrete type). Also new:
  `rt::block_in_place` and `CancelToken::cancelled_owned`.

- **`#[r2e::main]` / `#[r2e::test]` / `#[r2e::test_suite]` and
  `#[derive(BackgroundService)]` now emit facade paths** — the runtime is built
  through `<crate root>::rt::RuntimeBuilder` and the service token is
  `<crate root>::rt::CancelToken`, resolved through the same `r2e` /
  `r2e_core` root every other emitted path uses. **A generated project no
  longer needs `tokio` in its `Cargo.toml`** (`r2e new` stopped emitting it).
  `start_paused = true` needs the paused clock, now behind a forwarded feature:
  `r2e/test-util` → `r2e-core/test-util` → `r2e-rt/test-util`, which `r2e-test`
  turns on so it is present in any crate's dev graph and absent from release
  builds.

- **`clippy.toml`** grew a `disallowed-types` list —
  `tokio_util::sync::CancellationToken`, `tokio::task::JoinHandle`,
  `tokio::runtime::Handle` — next to the existing `disallowed-methods` deny on
  raw spawns. Runtime-neutral primitives (`tokio::sync::*`, `Instant`,
  `JoinSet`, …) stay allowed: they are re-exported by identity. The only
  exemptions are the `#[expect]`-marked wrapper definitions in `r2e-rt`.

- `r2e-events` (+ the `iggy` / `kafka` / `pulsar` / `rabbitmq` backends),
  `r2e-scheduler`, `r2e-executor` and `r2e-tenant` now go through the `rt`
  facade for spawning, timers, sync primitives and `select!`, and **dropped
  their direct `tokio` / `tokio-util` / `tokio-stream` dependencies**. No
  behaviour change; the four distributed backends needed no client-API escape
  hatch.

- **`r2e-http` re-sources the neutral HTTP types from the `http` crate** —
  `StatusCode`, `HeaderMap`, `HeaderName`, `HeaderValue`, `Method`, `Uri`,
  `Parts` and the header constants now come from `http::…` instead of
  `axum::http::…`, and `Extensions` / `Uri` likewise. **No type changes**: axum
  re-exports those very types from `http`, and the workspace resolves a single
  `http` version, so this is identity-preserving for every downstream signature
  — it only stops the workspace from calling `http` types "axum types". The
  `axum::` source baseline drops from 18 files / 32 occurrences to 9 files / 14
  occurrences, all inside `r2e-http/src/` (plan §5 step 3a). Steps 3b (R2E-owned
  `FromParts` / `IntoHttpResponse` traits) and 3c (a `Router` newtype) are
  deliberately **not** done — they are gated on the §5.3d decision about what
  users are promised.

- **The 11 example crates dropped their direct `tokio` / `tokio-util` /
  `tokio-stream` dependencies** and go through the facade like the framework
  does (`rt::sync::*`, `rt::sleep`/`rt::timeout`, `rt::select!`,
  `rt::TcpListener`/`rt::TcpStream`, `rt::io`, `rt::stream`, `#[r2e::test]`).
  With that the tokio dependency allowlist is exactly `{r2e-rt, r2e-test,
  r2e-devservices}` — the by-design set — and the tokio *source* baseline is
  empty workspace-wide.

- **r2e-observability**: `traced_reqwest_client` / `TraceContextMiddleware`
  now open an OpenTelemetry **client** span per outgoing request
  (`otel.kind = "client"`, name `HTTP {method}`, HTTP-client semantic
  conventions: `http.request.method`, `server.address`, `server.port`,
  `url.full`, `http.response.status_code`, `otel.status_code` /
  `error.message`) and propagate **that span's** context instead of the
  caller's. Tracing backends that derive a service graph from CLIENT→SERVER
  pairs (Tempo metrics-generator, Jaeger, Grafana) now show `caller → callee`
  edges and client-side latency for R2E services calling each other.
  Implemented on `reqwest-tracing` pinned to the workspace
  `opentelemetry 0.32` / `tracing-opentelemetry 0.33`. New re-exports:
  `R2eSpanBackend`, `OtelName`, `OtelPathNames`, `DisableOtelPropagation`.
  `inject_current_context` is unchanged (headers only, no client span).
  Follow-up of the outgoing-propagation work (#764, #765, #766); task #927.
