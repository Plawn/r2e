# R2E Roadmap

Status: **LIVE WORKING BACKLOG**. Only still-open work lives here — shipped
workstreams (DI/builder refactor, testing DX, plugin DX/DI overhaul, EventBus
perf W8, App-trait canonicalization W9, bean/controller unification W10,
OpenFGA schema-first W12 phases 1–3, …) were pruned on 2026-07-23 after
verifying each claim against the code; their record lives in the reference
docs (`plugins.md`, `beans-di.md`, `guards-interceptors.md`, `executor.md`,
`subsystems.md`, `di-builder-refactor.md`, `eventbus-perf.md`, `llm.txt`) and
in this file's git history.

## North star

R2E must **compound like Quarkus**: every feature plugs into DI, config,
testing, OpenAPI and observability with zero liaison code. Optimized for
humans **and** AI agents writing clean, well-architected, fast apps — the
idiomatic R2E path must always be the shortest, most discoverable path;
whenever a real app drops to raw axum or hand-rolls infrastructure, that is a
framework bug to record here.

---

## W2 — Framework gaps found in real apps → tracked in Tasker #635

Evidence base: the 2026-07-10 audit of two production-bound apps built on
pre-refactor R2E — **threaty** (~44K LOC, deep user: 20 controllers, ~101
routes, 138 injections, 48 path-parameterized guards, 3 custom
`Plugin`s) and **patina** (~23K LOC, shallow user: hand-built 10-field
state, registry-proxy core written as a raw axum fallback handler). Both leak
out of the framework at the same seams.

Tracked as umbrella task **#635 "R2E framework gaps from real-app audit
(threaty + patina)"** (target `r2e`), one sub-task per gap: EventBus↔SSE
bridge, proxy/streaming catch-all path, dynamic scheduled tasks, first-class
multipart, config-derive expressiveness, serve lifecycle / awaited drain,
auth-required without a phantom identity field, AI-facing DX. Full evidence
per gap in the Tasker sub-tasks and in this file's git history (`6d880f6`).
**All 8 sub-tasks and the umbrella #635 are CLOSED** (verified 2026-08-13) —
kept here only as the pointer to the evidence base.

## W4 — Plugin serve-path e2e audit — OPEN

The item-12 failure mode (gRPC `serve()` silently unwired) generalized: verify
every plugin's serve-time promise through the real `build_state → serve()`
path — prometheus, observability, oidc, openapi, static, scheduler, health.
One e2e test per plugin, in the spirit of `example-grpc/tests/grpc_serve.rs`.
The plugin `enabled` gate widens the surface: a disabled plugin's serve
promise must also be verified as *absent*.

Current state: each plugin crate has unit/integration tests, but none of them
boots the plugin through `build_state → serve()` and asserts the wire-level
promise.

## W6 — Testing DX follow-ups — OPEN

- Dev services for the remaining backends: **Kafka, RabbitMQ, Pulsar**
  (crate `r2e-devservices`, same workspace-session/Ryuk lifecycle as the
  shipped `DevPostgres`/`DevRedis`/`DevOpenFga`).
- ~~Demo dev-services usage in `example-postgres`~~ — SHIPPED 2026-07-23
  (`examples/example-postgres/tests/postgres_test.rs`: `DevPostgres::shared()`
  + isolated per-test database + `override_config_value("database.url", …)`).
- ~~Demo module-to-module composition (`imports(module(…))`) in an example
  app~~ — SHIPPED 2026-07-23 (`example-app` `OrderModule` imports
  `module(UserModule)`; `OrderService` injects the exported `UserService`;
  `tests/app/order_module.rs` proves the cross-module reach-across at runtime).
- `r2e doctor` check for missing dev-service config (deliberately NOT
  auto-sniffing config — implicitness hides failures).
- **Phase 3 (`r2e test --watch`): deferred, NOT approved** — do not start
  without an explicit user go.

## W11 — Carried from the root `todo` file — remainder

- **Zero-copy exploration (xitca-web)** — exploratory only: evaluate whether a
  zero-copy HTTP layer brings measurable wins over the current axum stack.
  No commitment.
- **Responsibility-boundaries audit (remainder)** — the scheduled/consumer
  half was absorbed by W10; what remains is a pass over which concern lives in
  which crate/macro (core vs http vs macros vs integrations).

## W13 — Live config × dev-reload — remainder

Shipped: `#[secret]` → `#[live_config]` rename + field injection on every host
(beans, decorator beans, background services, `#[bean]`/`#[producer]` params,
controllers), the rotation fix, and Phases 0–4 of the dev-reload × config
workstream — stable `LiveConfigRegistry` identity carried across hot patches
with a differential re-seed (B1/B3), `config_derived` never-pin set so typed
`ConfigProperties` beans refresh (B2), `ConfigKeyKind::{Required, Optional,
Section, Live}` with live keys out of the per-bean fingerprint (B5) and
`Section` keys hashing their whole prefix subtree, `ContextConstruct::
config_keys()` requalified as introspection-only (B6), and a boot-time WARN for
dead live keys. Reference: `docs/claude/dev-reload-config-semantics.md`.

Shipped 2026-08-13 (the three remaining tech-debt items):

- ~~**`bg_service_derive.rs` / `decorator_bean_derive.rs` emit no
  `config_keys()`.**~~ Both derives emit them now. The design question ("where
  does the declaration land for hosts that are not `Bean`s") resolved to *the
  host that owns the site aggregates*: `DecoratorSpec::config_keys()` folded
  into `Controller::validate_config` (`#[routes]`), into `Bean::config_keys()`
  (`#[bean]` intercept sites — those also reach the fingerprint), and into the
  new `GrpcService::validate_config` (`#[grpc_routes]`);
  `ServiceComponent::config_keys()` validated at `spawn_service` and, for
  `#[producer(start)]` outputs, during graph resolution. Shared bridge:
  `config::validate_declared_keys`. Closes Tasker #682 — a missing decorator
  `#[config]` key is now part of the aggregated startup report, never a
  fail-late panic in `build_decorator`.
- ~~**`#[derive(BackgroundService)]` config/live deps unchecked at compile
  time.**~~ `ServiceComponent` declares `type Deps` (BREAKING for hand-written
  impls) and `spawn_service` moved onto the `SpawnService` extension trait
  (prelude) so the witness is inferred — a service reading an absent bean is a
  compile error again.
- **B4 — half-fixed.** The real robustness gap (a watch that *ends* is never
  restarted, in prod as much as under `r2e dev`) is closed: watch tasks run
  under `config::supervise_config_watch` — `Err` restarts with capped
  exponential backoff, `Ok(())` is a documented "done, don't call me again",
  and the shutdown token is raced at every point of the cycle: before an
  attempt, **during** the in-flight `watch` future (`biased` select), and
  during the retry sleep — a provider whose `watch` never resolves cannot hold
  a graceful drain open. The dev-cycle re-spawn half is
  **deliberately deferred with a written design** (see
  `dev-reload-config-semantics.md` § "B4 — watch supervision"): it is not a
  correctness gap (one carried registry), Subsecond cannot patch a parked
  future anyway, and a correct re-spawn needs serve-token capture + per-cycle
  child tokens + provider dedup, testable only by serving across two cycles.

Audit follow-ups shipped 2026-08-13 (review of the above):

- **`#[producer(start)]` bypassed `ServiceComponent::Deps`.** The producer only
  demanded its own parameters, so a produced service reading an unprovided bean
  compiled and panicked in `from_context` at startup. `#[producer]` now folds
  `<Output as ServiceComponent>::Deps` into the producer's `Producer`/
  `Registrable` `Deps` via `TAppend` — compile error at `build_state()`.
  Covered by a trybuild fail + pass pair (`cases/executor/…`).
- **Producer-service and bean config errors are one report.** The service half
  used to `?` out several statements before the bean keys were checked;
  `BeanRegistry::validate_all_config` merges required bean keys, required
  service keys and declared service sections into a single
  `BeanError::MissingConfigKeys`. Side effect: config errors now precede
  missing-dependency errors in `resolve()`.
- **`#[config_section]` no longer fails late on late-built hosts.**
  `DecoratorSpec::config_sections()` and `ServiceComponent::config_sections()`
  return `Vec<SectionValidator>` (`SectionValidator::of::<C>(prefix)`, wrapping
  the same `validate_section::<C>` the controller meta module uses), run by
  `decorator_config_errors`, `try_spawn_service` and `validate_all_config`. A
  decorator/service section is now walked in full (missing nested keys, type
  mismatches, garde) at startup instead of panicking at construction.
  **Residual gap:** an `#[intercept]` site on a `#[bean]`'s
  `#[scheduled]`/`#[consumer]` method folds only the spec's `config_keys()`
  into the host `Bean::config_keys()`; that spec's *sections* are still
  validated when the decorator slot is filled inside `resolve()` — the same
  phase as a bean's own `#[config_section]` field, i.e. inside `build_state()`
  but not part of the aggregated report.
- **`try_register_grpc_service`** added (non-panicking peer of
  `try_register_controller`); `register_grpc_service` delegates to it.

Remaining:

- **Undeclared keys in hand-written beans stay stale on reuse.** A bean reading
  `config.get("x")` without listing `"x"` in `config_keys()` keeps a stable
  fingerprint and is reused with the old value. Documented and intended (declare
  the key, or use `#[config]`); listed here only so it is not "rediscovered" as
  a bug.

## W14 — Multi-tenant bean routing — SHIPPED 2026-08-14

Closes the last root-`todo` item ("avoir une feature pour router différents bean
en fonction du tenant, type plusieurs DB — infra générique puis implem
spécifique db"). Crate `r2e-tenant` (feature `tenant`, in `full`) + the
`tenant` feature on both data backends. Reference: `docs/features/24-tenancy.md`
(user guide), `docs/claude/subsystems.md` § Multi-tenancy (internals),
`examples/example-multi-tenant-db` (end-to-end).

Shipped:

- **Generic infra.** `TenantResolver` / `SyncTenantResolver` (SPI #1) with
  built-ins (`HeaderTenantResolver`, `PathTenantResolver`,
  `ExtensionTenantResolver`, `FnTenantResolver`); `TenantSource<T>` (SPI #2)
  with `TenantContext` cascade (`ctx.get::<U>()` resolves U **for the same
  tenant**, single-flighted, cycle-detected with a named chain);
  `Tenanted<T>` — single-flight create, no failure caching, bounded negative
  cache, `create-timeout`, idle/LRU sweep with `dispose`, drain on shutdown,
  `metrics()`/`stats()`/`evict()`/`invalidate()`/`preload()`.
- **Compile-checked wiring.** `Tenant<T>` / `TenantId` are `FromRequestPartsVia`
  + `ViaBean` (never axum `FromRequestParts` — pinned by
  `assert_unambiguous_extractor` probes), so a missing `Tenancy` / `PerTenant`
  plugin is a compile error at `register_controllers`, not a 500 on the first
  request from the first tenant. Compile-fail cases in
  `r2e-compile-tests/cases/tenancy/fail/`.
- **DB-specific impl.** `tenant-sqlx` / `tenant-diesel`: `TenantPools<..>`,
  `PoolSource` (tenant → DSN → pool), `TenantTx` — a `#[managed]` transaction on
  the requesting tenant's pool needing **no** controller field, because
  `TenantPool`'s `TxSource::Deps` list `TenantRouter` + `TenantPools<..>`.
- **`TenantId` parsed, never deserialized** — no `Deserialize` impl, so a value
  that picks a database/schema/bucket cannot arrive in a request body and skip
  validation.
- Config `tenancy.*` (precedence: `PerTenant` builder > file > default),
  `TenantError` → one status per failure mode (400/404/503/504/500, the
  request-driven three configurable), `tenancy.enabled: false` boots inert
  rather than requiring code deletion, test helpers `.as_tenant()` /
  `.as_tenant_user()`.

Deferred, with the reason (do not re-propose without addressing it):

- **Per-tenant migrations on the request path.** Documented as out of scope in
  both backends' `tenant` module docs. To be correct it belongs inside the
  single-flight cell (so N concurrent first requests migrate once), which puts
  it under `tenancy.create-timeout` — a migration set slower than that budget
  surfaces as a 504 for whichever tenant triggered it. Until that interaction is
  designed, tenants migrate from the provisioning path.
- **B4-style watch re-spawn / distributed negative cache.** The negative cache
  and the sweep are per-process; a multi-process deployment remembers unknown
  tenants independently. Fine at `negative-ttl` scale, would need a shared
  backend to be more.
- **No `Tenanted` Prometheus exporter.** `TenantedMetrics` / `TenantStats`
  implement `Serialize` for admin JSON (`TenantStats::idle` becomes `idle_ms`),
  but metrics-exporter wiring remains application-owned.
- **`#[inject(request)]` is still not modeled in OpenAPI**, so `Tenant<T>` /
  `TenantId` fields do not appear in the spec. Pre-existing gap, not
  tenancy-specific.
- **No CLI surface** (`r2e generate` scaffolding for a resolver/source).

### Audit status

An external read-only review of `f41d015` (2026-08-14) is recorded verbatim in
`docs/claude/w14-tenancy-audit.md`, with its triage/fix status at the top.
Findings 1/4/9, 2/5/6, 7 and 8 were addressed on 2026-08-14. Finding 3 is an
accepted, documented limitation: concurrent-root cascade cycles end at
`create-timeout` (504), or hang when the timeout is disabled.

### Remaining DX frictions observed while building W14

Collected as they were hit, with resolved audit items removed. Ordered by how
often a user would hit it.

- **`TenantTx` reads differently on the two backends** — `TenantTx<'_, DB>`
  (sqlx) vs `TenantTx<Conn>` (diesel, no lifetime), because the two `ManagedTx`
  types differ. The most likely copy-paste error between backends. Same story
  for `PoolSource::with_options` (sqlx `PoolOptions` is `Clone`) vs
  `with_factory` (r2d2 `Builder` is not).
- **`Tenant<T>` only works as a controller field** (`#[inject(request)]`), never
  as a handler parameter: `#[routes]` accepts only `#[inject(identity)]` on
  params, and a plain param must implement axum's `FromRequestParts`, which
  `Tenant<T>` deliberately does not. Mostly-public controllers that want one
  tenant-aware route must still declare the field. Fixing it means teaching the
  macro `#[inject(request)]` on params (would also benefit every other
  request-scoped type).
- **`TenantId` is `Serialize`-only**, which is right for request bodies but also
  blocks tests from deserializing a response that contains one. Tests compare
  strings today. A test-only or feature-gated `Deserialize` would remove that
  without weakening the "never from a body" property — the property that matters
  is that *extraction* validates, so a `Deserialize` that goes through `parse`
  may be acceptable.
- **`r2e-tenant`'s own tests cannot use `TestApp`** (it lives in `r2e-test`,
  which depends on the facade → dev-dependency cycle), so they drive
  `tower::ServiceExt::oneshot` over a built `Router`. Same constraint as
  `r2e-core`'s controller tests. Consequence: the `TestApp` path for tenancy is
  covered only from the backend crates and the example.
- **Evicted diesel pools are not closed**, only dropped — r2d2 has no close, so a
  tenant's connections linger until in-flight ones finish. `max-active` is a
  soft trim target, so `max-active × max_connections` is only a steady-state
  planning target, never a hard burst bound.
- **`Tenancy::resolver::<R>()` needs a type-state hop** (`Tenancy<()>` →
  `Tenancy<R>`) because `R` must appear in `Deps`; a plain inherent method on
  `Tenancy<R>` is E0107. Works, reads slightly oddly. `PerTenant`'s
  `fallback_to_default()` uses the same trick for a better reason (it adds `T` to
  `Deps`).
- **`DevPostgres::create_database(name)` was skipped** — one container backing N
  tenant databases needs either a SQL client dependency in `r2e-devservices` or
  container `exec` plumbing. Until then, Docker-based per-tenant tests create
  their own databases.
- **Generated handlers now carry `#[allow(clippy::too_many_arguments)]`** because
  the 6-extractor head prefix pushes them past clippy's threshold; without it
  every downstream crate using `#[managed]` or `#[guard]` gets a new warning.
  Hiding a lint in generated code is a smell even when it is the right call.

Accepted limitations and remaining frictions from the external audit:

- **`Tenant<T>::into_inner` / `into_parts` let a request-scoped resource escape**
  the request with no lease semantics. This is now the documented contract;
  resource handles must tolerate close-while-cloned.
- **`max_negative` is global only** — every other bound (`max_active`, TTLs,
  create timeout) is also a per-resource builder method.
- **sqlx `PoolSource::new` hands the closure an owned `TenantId`** while
  `TenantSource::create` receives `&TenantId`; custom directory code pays an
  avoidable clone/signature mismatch.

## W15 — Factory-first plugins — SHIPPED 2026-08-15

`Plugin` fully integrated into DI: a plugin IS one async fallible
factory for its `Provided` tuple, executed inside `build_state()` as a bean
graph node (topologically after `Deps`, config guaranteed loaded).
`install`/`configure` deleted; optional `setup()` remains the rare pre-graph
hook. Reference: `docs/claude/plugins.md` (authoritative, fully rewritten).

Shipped:

- **New trait surface**: `build(self, deps, config, &mut PluginBuildContext)
  -> Result<Provided, PluginBuildError>`; `Err` → `BeanError::PluginBuild`.
  `PluginInstallContext` → `PluginSetupContext` (loses `config()`/
  `config_get`/`run_post_construct`). `GraphHandle` (Late-backed handle on the
  final `Arc<BeanContext>`) for post-boot resolution (used by `Tenanted<T>`).
- **Two orders, documented**: build execution = topo order; effect application
  (`add_layer`/`wrap_router`/`on_serve`/…) = install order. `<prefix>.enabled=
  false` drops the **surface** lane only (layers, routes, serve hooks); the
  **cleanup** lane (`on_shutdown`/`on_shutdown_async`) still applies, so a
  plugin disposes of whatever its `build` constructed. `build` always runs
  (return a disabled variant). Config section parsed even when disabled.
- **Strict registration**: plugin beans collide with app `.provide()` or a
  double install → `DuplicateBean` at boot. Pin-before-install wins;
  all-pinned skips `build` only for a plugin that opts in with
  `const SKIP_BUILD_WHEN_ALL_PINNED: bool = true` (default `false` — effects
  are not beans and cannot be pinned); partial pin always runs it, and the
  effects then resolve their beans from the graph, not from what `build` made.
- **All 10 in-tree plugins migrated.** OpenFga is the flagship: `Late<GrpcBackend>`,
  `LazyBackend`, `PostConstruct` boot smuggle, `NotReady` (→ `Disabled`), and
  the "install after load_config" panic all deleted. Tenancy/PerTenant build
  directly wired (no `unwired()`/`wire()`). Executor now honors `executor.*`
  regardless of `.plugin()`/`load_config()` order (killed the example-app
  latent bug; assertion test in example-app/tests/app/app.rs).
- `Late<T>` remains public as a de-emphasized escape hatch.

Post-audit hardening (same branch): setup context lost every surface-effect
*sugar* — no `add_layer`/`wrap_router`/`on_serve`/shutdown hooks, so a disabled
plugin can no longer mount a route through sugar; the raw `add_deferred` escape
hatch remains and is documented as unconditional (it hands out the full
`DeferredContext`, which the framework cannot gate — that is the caveat, not an
oversight); the router's graph `Arc` now rides the request future *and* the
response body (tower's `oneshot` drops the service before the first poll, hyper
drops the head while the body streams); **tracked work owns the graph while it
runs** — `ServeContext::track` now takes the *future* (breaking; it spawns and
wraps) and `spawn_service` / the scheduler driver / the QUIC drain go through
the same `ServiceHandles::spawn_owning`, so an elapsed `shutdown_grace_period`
or a dropped `run()` future (hot patch) — the paths where nothing joins the
handle — still leave the task with a live graph — plus **`run()` now cancels the
shutdown token and drains the tracked handles on the abort paths too** (a
startup-hook `Err`, a serve error: `prepared.rs::abort_started_work`), so a task
that waits on the token is no longer stranded with its port and its graph; plus
`PreparedApp` holds a strong `Arc` for the whole serving lifecycle (shutdown
hooks, in-flight WebSocket sessions);
`with_state(())` + `.plugin()` is a documented `debug!`
no-op instead of a `debug_assert`; shutdown hooks split off from the surface
lane so a disabled plugin still disposes; Prometheus no longer installs the
global recorder when disabled; Scheduler **and** PerTenant effects resolve their
beans from the graph at apply time (partial pins); dev-reload seeds
`forced_rebuild` with volatile (plugin) nodes so their dependents cannot keep a
previous cycle's instance.

Residual (accepted caveat): a WebSocket session is not part of graceful drain —
on upgrade hyper's `UpgradeableConnection` hands the IO off and returns
`Ready(Ok(()))`, so the connection counts as finished and axum's detached
`on_upgrade` task is unwatched. Sessions keep graph access for the whole of
`run()` (the serve-scope `Arc`), but a session still alive *after* `run()`
returns has none. In a normal binary the runtime is dropped right after, taking
those tasks with it; an embedder that keeps the runtime alive past `run()` must
resolve what a session needs before its socket loop. Rejected fix: inserting the
`Arc` into request extensions so the generated `on_upgrade` closure could
capture it — a boxed insert on *every* request to serve the WS-only case.

Deferred: per-plugin effect caching under dev-reload (volatile nodes rebuild
every hot-patch cycle — matches the old fresh-install-per-cycle semantics);
Prometheus keeps its global recorder when enabled (separate workstream).

Known dev-reload gaps (diagnosed, not fixed — `r2e dev` only, all pre-existing):

1. **Startup lifecycle is skipped once initialized** (`builder/prepared.rs`,
   the `if !skip_lifecycle` block). Deliberate for consumer subscriptions and
   serve hooks (re-running them would double-subscribe), but it also means a
   controller `#[post_construct]` never re-runs although controller cores are
   rebuilt every cycle, and anything a patch *adds* (a new `#[consumer]`, a new
   `#[scheduled]`, a new startup hook) never starts until a full restart.
   Tractable next step: hoist the controller `#[post_construct]` loop out of the
   guard; the additive case needs a diff of registrations between cycles.
2. **The dropped server future runs no shutdown sequence** (dioxus-devtools
   drops it on patch). Plugin `on_shutdown*`, drain hooks and `#[pre_destroy]`
   never fire between cycles, and the future's `drop_guard` cancels that cycle's
   `cancel_token`, so anything keyed off `ServeContext::shutdown_token()` (the
   live-config watch supervisor, tracked tasks) dies without being restarted.
   Since the app token exists before serving (lazily created and memoized in
   `plugin_data`) and every framework token is a `child_token()` of it, this now
   covers serve-scope work uniformly —
   `spawn_service` background services and the scheduler driver (which relays
   the app token onto its own) stop on a patch too, where previously they
   survived because only a shutdown hook cancelled them. Deliberate: a stranded
   task from cycle N-1 is worse than a stopped one, and item 1 already says
   nothing serve-scope restarts in later cycles.
3. Consequence of (2) plus the correct graph release: cycle N-1's tracked tasks
   are cancelled but joined by nobody, so each holds its own cycle's graph until
   it returns; once the last one does, cycle N-1's graph is really dropped and
   any instance carried over that still points at it reads empty. Item 1 of the
   volatile fix above closes the plugin-bean shape of this; a bean carrying a
   raw `GraphHandle` obtained by other means is still exposed.

## W12 — OpenFGA DX — Phase 4 (CLI), lowest priority

Phases 1–3 shipped 2026-07-20 (`.fga` parser + `model!` typed API, typed
`FgaClient` with write-through invalidation, `OpenFga` plugin owning the store
lifecycle at boot). Reference: `docs/features/23-openfga.md`.

Remaining: `r2e fga diff` / `push` / `pull` (diff local model vs store, pull an
existing store's model into a local `.fga`), plus tuple seed fixtures for
dev/tests. Nothing FGA-related exists in `r2e-cli` yet beyond the bundled doc.

## W16 — MCP server (r2e-mcp) — P1+P2 SHIPPED 2026-08-27, P3 OPEN

P1 (server core) shipped on `feat/mcp-server`: `r2e-mcp` crate + `McpServer`
plugin (streamable HTTP, shared sessions across SO_REUSEPORT workers,
shutdown-token relay), `#[mcp_routes]` + `#[tool]` (schemars schemas, guards
SHARED with HTTP via `Guard<I>`, prebuilt interceptors, `EndpointDeps` compile
check), example-mcp, full test target + compile-fail cases. References:
`docs/features/25-mcp.md` (user guide), `docs/claude/transport-adapters.md`
(guards rule-of-three reversal), plan
`~/.claude/plans/j-aimerai-tudier-l-id-e-d-avoir-typed-tower.md`.

P2 (auth) shipped on the same branch: IdP-agnostic OAuth 2.1 resource server
(`mcp.auth.*`) — jwt backend over discovery (RFC 8414 incl. path-insertion,
TTL + stale-if-error), `McpAuthLayer` (401/403/503 + exact `WWW-Authenticate`
challenges), RFC 9728 protected-resource metadata, static DCR shim
(`public-client-id` + redirect allowlist + mirrored AS metadata), per-tool
`#[tool(scopes/any_scopes)]` + shared `#[roles]` guards + `tools/list`
filtering, `ScopePolicy` (scope/scp/permissions ladder, Keycloak realm/client
roles), `server.public-url` convention key, `TestJwt::for_resource` +
`TokenBuilder::{scopes,audiences,realm_roles,client_roles}` +
`pin_mcp_validator` (feature `testing` / facade `mcp-testing`), r2e-security
`audiences`/`skip_audience_validation`/`with_leeway`. Auth test target (64
tests) + example-mcp auth e2e; provider matrix + Keycloak walkthrough in
`docs/features/25-mcp.md`.

Open, in order:

- **P3 — providers & dev**: MCP resources + prompts.
  SHIPPED 2026-08-27: the `introspection` (RFC 7662) + `userinfo` (Google)
  validation backends (`token-validation: introspection|userinfo`,
  opaque-token cache `opaque-cache-ttl-secs`/`opaque-cache-max-entries`,
  `r2e_mcp::auth::{IntrospectionBackend, UserinfoBackend}`) and the
  authorize-redirect shim (`extra-authorize-params` → mirror rewrites
  `authorization_endpoint` to `{mcp.path}/oauth/authorize`, 302 to the IdP,
  server params win; requires the DCR shim, boot error otherwise); r2e-oidc
  RFC 8707 `resource` pass-through (`resource` form param → token `aud`,
  invalid URI = 400 `invalid_target`; `scope` pass-through already existed);
  `DevKeycloak` (r2e-devservices feature `keycloak`: `start-dev
  --import-realm`, bundled `r2e-mcp` realm with audience-mapped `mcp` scope,
  `password_token`/`client_token`/`admin_token`; realm JSON is part of the
  shared-container identity — copy sources are digested, no discriminator
  needed) + Docker-gated e2e in `r2e-mcp/tests/auth/keycloak.rs`. Two fixes
  the real container surfaced: a full realm import creates ONLY the client
  scopes listed in the JSON (Keycloak's built-ins `basic` → `sub` and
  `roles` → `realm_access` must be defined explicitly or tokens carry
  neither), and r2e-security narrowed `Validation.algorithms` to the token's
  algorithm at decode — jsonwebtoken 10 rejects any list mixing key families
  (RS256+ES256 default) once the key is known (`InvalidAlgorithm` on every
  token).
- **Follow-ups (any phase)**: targeted error for struct-level
  `#[inject(identity)]` on `#[mcp_routes]` types (points at the method-param
  form); bean-level `#[tool]` auto-collection (`after_register`, like
  `ScheduledSource`); r2e-oidc authorization-code + PKCE flow (Docker-free
  end-to-end MCP OAuth in `r2e dev`); `r2e add mcp` CLI scaffold; sealed
  `ObjectParams` marker making a non-object root schema a compile error.

## Open items tracked in their own docs

Kept where the context lives rather than duplicated here:

- `plans/phase1-optional-deps-conditional-beans.md` — example-app demo of the
  config-driven `#[producer] -> Option<T>` pattern.
- `plans/phase2-profiles-alternatives.md` — `#[bean(profile = "…")]` sugar
  (open design conflict with `P`), guaranteed profile groups, two profile test
  gaps (`R2E_PROFILE` precedence, `"default"` fallback).
- `docs/claude/eventbus-perf.md` — P4.4 Kafka consumer multiplexing, Kafka
  blocking drain commit, Iggy producer batching, failure-injection/redelivery
  tests + throughput bench.
- `docs/research/HANDOFF-perf-tpc.md` — Linux benchmark run, proxy-mesh push
  gate, tunnel `copy_bidirectional` (543).
- `docs/research/thread-per-core.md` — stall detector, `#[offload]`/
  `#[blocking]`, `threads_per_worker` (none implemented).

---

## Decisions log — do NOT re-propose

- **Qualifiers / named beans: REJECTED.** Newtypes are the chosen pattern for
  same-typed beans (runtime `DuplicateBean` backstop).
- **`#[transactional]`: REMOVED (W10 phase 4, 2026-07-16, user-approved).**
  `#[managed]` is the single transaction story. The body wrapper had zero
  usage, relied on an unhygienic magic `tx` variable injected into the body
  scope, and every doc already said "prefer `#[managed]`". Do not reintroduce
  it — extend `ManagedResource` instead if a gap shows up.
- **`AppBuilder::register_subscriber`: REMOVED** — `#[consumer]` beans are
  auto-collected at `build_state()`.
- **Pre-state plugin `Deps`: REMOVED (2026-07-21).** ONE `Deps` list, appended
  to `R`, verified at `build_state()`, resolved at `configure`; `install` has
  no deps parameter. `r2e_core::Late<T>` covers "provided bean needs a dep".
- **`Guard::startup_check`: permanently superseded** by compile-checked
  decorator deps.
- **Scheduled-method interceptors run on DIRECT calls too** (user decision:
  an admin route calling `self.tick()` keeps audit/logging); gRPC stays
  entry-point-only. Sync scheduled methods with interceptors are promoted to
  `async fn` (`block_on` and fire-and-forget spawn were analyzed and
  rejected).
- **No "ambient beans"**: cross-cutting infra beans are imported explicitly
  per module.
- **Test overrides are pinned (first-wins)**, not last-wins: the harness
  pre-configures the builder before the blueprint runs, so overrides must
  beat later registrations.
- **Dev-reload re-reads `application.yaml` per patch** (deliberate: config is
  not pinned across hot-patches, unlike `.provide()`-ed beans).
- **Per-transport guards until a third wire exists** (rule of three);
  `GrpcRolesGuard`≈`RolesGuard` ~30-line duplication accepted.
- **Dev services are explicit** (`DevPostgres::shared()`), never
  config-sniffed.
- **Bean interception is Quarkus-style, opt-in via `#[bean]` on the struct**
  (user decision 2026-07-16): direct in-code calls run the chain too (slot
  field injected by the struct attribute). The Spring-style "ticks/events
  only" fallback was considered and rejected — no silent semantic split.
  Accepted DX cost: struct literals outside the `#[bean]` impl block (and
  field-enumerating derives) need the hidden `__r2e_decos` field.
- **Pinned override = undecorated** (user decision 2026-07-16): pinning a
  bean (`override_bean`) skips ALL its hooks — post_construct, scheduled
  sources, and the decorator fill. One rule, no exceptions. Canonical test
  pattern: pin the *dependencies*, not the decorated bean, so the graph-built
  bean keeps its interceptors while IO is faked. **Explicit opt-ins added
  (2026-07-16, default unchanged):** `Decorate::decorate(ctx)` (blanket
  extension trait over `BeanDecoFill`, not in the prelude) fills a hand-built
  instance's slot from a resolved graph; `.override_bean_decorated(instance)`
  pins AND queues the deco fill (decoration only — the pin's dropped scheduled
  tasks / skipped `#[post_construct]` stay that way).
- **OpenFGA `list_objects`: DROPPED from the typed surface (user decision
  2026-07-20).** `ListObjectsResponse` is a bare `repeated string objects` —
  server-side bounds (`OPENFGA_LIST_OBJECTS_MAX_RESULTS`, deadline) silently
  return a *partial* list with no truncation flag or cursor, so a typed
  `Vec<FgaObject<T>>` would read as exhaustive without being it. Revisit only
  on real need, in this order: paginate-app-objects + `BatchCheck` helper
  (best candidate), `StreamedListObjects`, `Read`-paginated helper.
- **OpenFGA write-through invalidation is exact-object only**; transitive
  fan-out (userset grants) needs `clear_cache()`/TTL, and the
  invalidate-after-write TOCTOU is documented on the registry rather than
  fixed with cache versioning. OpenFGA `Write` semantics kept verbatim
  (duplicate grant / missing revoke = server error, not a no-op).
- **OpenFGA model DSL is the compile input** (`.fga`), not JSON — requiring a
  `fga model transform` pre-step breaks the promise. Conditions (schema 1.2):
  parser-tolerant CEL passthrough only in v1.
