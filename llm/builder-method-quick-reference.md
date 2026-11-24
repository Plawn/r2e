---
topic: builder-method-quick-reference
features: core
tokens: ~1600
requires: app-builder, dev-experience
---

## Builder Method Quick Reference

### TL;DR

- `.plugin(p)` is the single install call for every plugin, and it always runs before `build_state()`.
- Order is builder phase (`load_config`, `provide`, `register`, `register_module`, plugins — mutually order-independent) → `.build_state().await` → app phase (`register_controller(s)`, lifecycle hooks) → a terminal `serve`/`build`.
- `.when(cond, |b| ...)` composes conditional assembly but canNOT wrap `.plugin()`.
- Do not call a terminal `serve_auto()` from `App::build` — the entry point does it (see llm/dev-experience.md).

| Method | Phase | Purpose |
|--------|-------|---------|
| `.plugin(p)` | builder | **Every** plugin — the only install call (Health, Cors, HttpTrace, Tracing [subscriber only], NormalizePath, SecureHeaders, RequestIdPlugin, DevReload, OpenApiPlugin, EmbeddedFrontend, Executor, Scheduler [requires Executor], SqlxDataSource/DieselDataSource, DataSourceHealth, GrpcServer, OidcServer, Prometheus [or MetricsFacade, feature `metrics-facade`], Observability, OpenFga, Tenancy/PerTenant) |
| `.with_config_provider(p)` | builder | Register an external `ConfigProvider` before `load_config` |
| `.load_config::<C>()` | builder | Load YAML+env; auto-register `C`'s sections as beans (sole disk-reading config registration point) |
| `.provide_config(c)` | builder | Provide a typed config value already in hand + register its nested sections as beans |
| `.r2e_config()` | builder/app | Borrow the loaded `R2eConfig` for a **non-DI** read (tests, `main`). NOT for building plugins: a config-driven plugin declares `type Config` + `CONFIG_PREFIX` and receives the section in `build` (`Cors::new()` ← `cors.*`, `OpenApiPlugin::default()` ← `openapi.*`) — never cut the chain to read it before `.plugin(..)` |
| `.override_config(cfg)` | builder | Test harness: stash an in-memory `R2eConfig` the next `load_config` uses instead of disk (`override_config_value` still wins) |
| `.provide(value)` | builder | Provide a constructed bean |
| `.register::<T>()` | builder | Register a Bean / AsyncBean / Producer |
| `.register_module::<M>()` | builder | Feature module (providers + controllers) |
| `.provide_with_post_construct(v)` | builder | `.provide(v)` + run `v`'s `PostConstruct` hook during `build_state` (after factory beans' hooks) |
| `.with_bean_factory(\|cfg: &R2eConfig\| B)` | builder | Bean built by a closure over the loaded config (prefer a `#[bean]` with `#[config]` params) |
| `.with_default_bean::<B>()` / `.with_default_async_bean::<B>()` / `.with_default_producer::<P>()` | builder | Overridable default registration (library/plugin side) |
| `.register_override::<T>()` | builder | Replace a `with_default_*` registration of the same type (last-wins; does not add a second slot) |
| `.with_profile(p)` | builder | Force the active profile (beats `R2E_PROFILE` / `r2e.profile`); tests use it instead of mutating env |
| `.with_config_file(path)` | builder | Base config file other than `application.yaml` for the next `load_config` (profile overlay derived from it) |
| `.when(cond, \|b\| ...)` | any | Conditional `Self -> Self` assembly (`config_flag`, `profile_is`) — canNOT wrap `.plugin()` |
| `.build_state().await` | — | Resolve graph → inferred HList state, wrapped as `BeanState<L>` (no type args) |
| `.register_controller::<C>()` | app | Register controller (+ consumers + scheduled) |
| `.register_controllers::<(A, B)>()` | app | Several at once |
| `.bridge_sse::<Bus, E>()` | app | EventBus → SSE topic bridge |
| `.schedule_task(def)` | app | Dynamic scheduled task |
| `.with_layer(l)` / `.with_layer_in(slot, l)` | app | App-wide tower layer (`RouteLayer`); lands in the `LayerSlot::App` band (innermost) unless a `LayerSlot` is given — band, not call order, decides position (see llm/plugins.md) |
| `.with_layer_fn(f)` / `.with_layer_fn_in(slot, f)` | app | `FnOnce(r2e::http::Routes) -> Routes` transform of the assembled routes, in `LayerSlot::App` by default; `.with_service_builder(f)` is an alias of `with_layer_fn` |
| `.merge_router(r)` | app | Merge hand-built `r2e::http::Routes<T>` (a raw axum `Router` must go through `r2e::http::axum_compat::from_router` first); no controller DI/guards/interceptors |
| `.with_meta_consumer::<M, _>(f)` | app | At `build()`, `f(&[M])` reads every collected route-metadata item `M` and returns `Routes<T>` to merge (plugin authoring) |
| `.get_plugin_data::<D>()` | app | Read data a plugin stored with `DeferredContext::store_data` (`Option<&D>`) |
| `.on_panic(f)` | any | App callback per caught panic — HTTP handlers, `#[scheduled]` ticks, executor jobs (`PanicReport` = message + `origin()` + `label()`). R2E logs + contains either way; this is the counting seam |
| `.on_start(f)` / `.on_stop(f)` | app | Lifecycle hooks (`on_stop` always runs at shutdown, outside every budget) |
| `.on_start_once(f)` | app | Like `.on_start`, but once per **process**: skipped on later `r2e dev` hot-patch cycles |
| `.on_drain(f)` | app | Awaited hooks run at shutdown trigger, BEFORE the listener stops accepting |
| `.drain_timeout(d)` | app | Bound the HTTP drain (in-flight requests after the listener stops). Default: 30s, or `server.drain-timeout` |
| `.with_stop_handle(h)` | any | Wire a hand-made `StopHandle` into shutdown (rare: a provided `StopHandle` bean is picked up automatically) |
| `.drain_timeout_unbounded()` | app | Opt out of the drain bound entirely (plain-axum behaviour); wins over config |
| `.shutdown_grace_period(d)` | app | Bound the tracked-handle join, PER handle (`spawn_service`, `ServeContext::track`, `#[ws]` sessions). Default: unbounded |
| `.per_worker_service(f)` | any | Shard-local `!Send` service per sharded worker (`WorkerContext` → `WorkerService`); needs `server.workers` |
| `.worker_local(f)` | pre-state | `.provide(WorkerLocal<T>)` + install as per-worker service: exactly one `T` per worker (`T` may be `!Send`) |
| `.serve(addr)` / `.serve_auto()` / `.build()` | terminal | Listen / get `Routes` (usually not called by hand — `r2e::launch!` calls `serve_auto`) |

Launching: `main.rs` is normally just `r2e::app_main!(MyApp);` — the entry point
performs `AppBuilder::new()` … `serve_auto()` around your `App::build`, so
`build` returns the `BootableApp` and never calls a terminal serve itself.
See llm/dev-experience.md.
