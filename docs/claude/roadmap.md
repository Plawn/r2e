# R2E Roadmap

Status: **LIVE WORKING BACKLOG** — only still-open work and the decisions log
live here. Shipped workstreams (what landed, when, what was deferred and why)
are recorded in `docs/claude/roadmap-archive.md`, one section per workstream
(W2 real-app audit, W13 live config, W14 tenancy, W15 factory-first plugins,
W16 MCP P1–P5, W17 data-catalog sprints, tracing single path); the reference
docs (`plugins.md`, `beans-di.md`, `guards-interceptors.md`, `executor.md`,
`subsystems.md`, `di-builder-refactor.md`, `eventbus-perf.md`, `llm.txt`)
stay authoritative for how things work today. When a workstream ships, move
its narrative to the archive and leave only the open remainder here.

## North star

R2E must **compound like Quarkus**: every feature plugs into DI, config,
testing, OpenAPI and observability with zero liaison code. Optimized for
humans **and** AI agents writing clean, well-architected, fast apps — the
idiomatic R2E path must always be the shortest, most discoverable path;
whenever a real app drops to raw axum or hand-rolls infrastructure, that is a
framework bug to record here.

---

## W4 — Plugin serve-path e2e audit — OPEN (Tasker #1365)

The item-12 failure mode (gRPC `serve()` silently unwired) generalized: verify
every plugin's serve-time promise through the real `build_state → serve()`
path — prometheus, observability, oidc, openapi, static, scheduler, health.
One e2e test per plugin, in the spirit of `example-grpc/tests/grpc_serve.rs`.
The plugin `enabled` gate widens the surface: a disabled plugin's serve
promise must also be verified as *absent*.

Current state (checked against the code 2026-11-12): no plugin crate boots its
own plugin through `build_state → serve()`. What exists is indirect:

- `example-app/tests` (real boot through `TestApp::boot`) asserts `/health`,
  `/openapi.json`, `/docs` and the scheduler on the wire.
- `r2e-core/tests/plugin/enabled.rs` covers the `enabled` gate generically
  (inert variant, effects dropped, disposal) with test plugins.

Still missing: a wire-level test for **prometheus** (`/metrics` is only named
in an `exclude_paths` list), **observability**, **oidc** and **static**, and
the *absent when disabled* assertion for every real plugin.

## W11 — Carried from the root `todo` file — remainder

- **HTTP backend containment** — `plans/axum-containment-roadmap.md`: phases A
  (one serve path, `r2e_http::server::serve`) and B (tower-http behind
  `r2e_http::layers`) and C (`Routes<S>` replaces `Router`, breaking) shipped
  2026-10-25, released as r2e 0.9.0 / r2e-core 0.8.0 / r2e-http 0.7.0
  (PR #113). Optional follow-up: `hyper_util` `auto::Builder` tuning —
  `r2e-http/src/server.rs` still serves through `axum::serve`, so tuning
  means owning the accept loop there first. A backend swap (ntex/tako/xitca) is explicitly
  NOT on the roadmap; its trigger is written in that file (§3 phase D).
- **Responsibility-boundaries audit (remainder)** — the scheduled/consumer
  half was absorbed by W10; what remains is a pass over which concern lives in
  which crate/macro (core vs http vs macros vs integrations).

## W13 — Live config × dev-reload — remainder

Shipped work (rename to `#[live_config]`, dev-reload phases 0–4, the three
tech-debt items and the audit follow-ups of 2026-08-13): archive § W13.
Reference: `docs/claude/dev-reload-config-semantics.md`.

- **Undeclared keys in hand-written beans stay stale on reuse.** A bean reading
  `config.get("x")` without listing `"x"` in `config_keys()` keeps a stable
  fingerprint and is reused with the old value. Documented and intended (declare
  the key, or use `#[config]`); listed here only so it is not "rediscovered" as
  a bug.
- **Residual gap of the `#[config_section]` validation:** an `#[intercept]`
  site on a `#[bean]`'s `#[scheduled]`/`#[consumer]` method folds only the
  spec's `config_keys()` into the host `Bean::config_keys()`; that spec's
  *sections* are still validated when the decorator slot is filled inside
  `resolve()` — inside `build_state()` but not part of the aggregated report.
- **B4 dev-cycle watch re-spawn: deliberately deferred with a written design**
  (`dev-reload-config-semantics.md` § "B4 — watch supervision"): not a
  correctness gap (one carried registry); a correct re-spawn needs serve-token
  capture + per-cycle child tokens + provider dedup.

## W12 — OpenFGA DX — Phase 4 (CLI), lowest priority (Tasker #1366)

Phases 1–3 shipped 2026-07-20 (`.fga` parser + `model!` typed API, typed
`FgaClient` with write-through invalidation, `OpenFga` plugin owning the store
lifecycle at boot). Reference: `docs/features/23-openfga.md`.

Remaining: `r2e fga diff` / `push` / `pull` (diff local model vs store, pull an
existing store's model into a local `.fga`), plus tuple seed fixtures for
dev/tests. Nothing FGA-related exists in `r2e-cli` yet beyond the bundled doc.

## W14 — Multi-tenant bean routing — remainder

Shipped 2026-08-14 (crate `r2e-tenant`, `tenant` feature on both data
backends, audit fixes): archive § W14. Reference: `docs/features/24-tenancy.md`
(user guide), `docs/claude/subsystems.md` § Multi-tenancy (internals).

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

## W15 — Factory-first plugins — remainder

Shipped 2026-08-15 (plugin = one async fallible factory inside `build_state()`,
post-audit hardening, the WebSocket drain caveat): archive § W15. Reference:
`docs/claude/plugins.md`.

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

## W16 — MCP server (r2e-mcp) — remainder

P1–P3 (server core, OAuth 2.1 resource server, providers/dev + per-session
members), P5 (`McpRegistry`), the mount-stage/identity contract (#1175) and
the loopback transport (#1076) are shipped: archive § W16. References:
`docs/features/25-mcp.md` (user guide), `docs/claude/transport-adapters.md`
(adapter internals + decisions log).

- **P4 — protocol gaps: done.** Progress, live elicitation, `completion/complete`
  and list pagination shipped 2026-09-26; MRTR (SEP-2322, elicitation for
  sessionless 2026-07-28 clients) merged 2026-11-12 via PR #151 (`7a46300f`):
  keyed `McpClient::elicit(key, …)` (breaking), opt-in through
  `mcp.request-state-key`. SEP-2577-deprecated sampling, roots and logging are
  deliberately NOT implemented. Plan: `plans/w16-p4-mcp-protocol-gaps.md`.
- **Awaiting release:** MRTR only — a breaking bump of `r2e-mcp`
  (`bump-version.sh r2e-mcp breaking`, propagates to `r2e`). The loopback
  transport #1076 and the session idle timeout + `McpSessionStore` #1244
  shipped in `r2e-mcp` 0.12.0 (2026-11-10); both tickets are closed.
- In-process MCP test client (#1245): **done, awaiting merge/release** —
  `r2e_mcp::testing::McpTestClient` (`app.mcp()` via the `McpTestExt`
  extension trait, feature `testing`), additive on `r2e-mcp`.
- `McpSessions::each`/bulk ops and per-tenant filtering, if a real app asks
  for them.
- Hexagonal / generator follow-ups #1216–#1220: see "Standalone Tasker
  tickets" below.

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

## Standalone Tasker tickets (target `r2e`, open as of 2026-11-12)

Not part of a workstream above; Tasker is the record, this list is the
pointer. Verified against Tasker and the code on 2026-11-12 (master
`1676ab0c`). r2e 0.12.0 (2026-11-10, PR #149) released #1076, #1244, #1181,
#1173, #1288, #1290, #1291 and #1299 — all closed. "Merged" below means on
`master`, unreleased: the ticket closes with the release that ships it.

- **Next release (nothing cut yet):** `r2e-mcp` breaking (MRTR, PR #151 →
  propagates to `r2e`), `r2e-macros` + `r2e-openapi` patch (#1272), `r2e-core`
  patch (request ids on `uuid` `fast-rng`, PR #150).
- **#1272 (medium)** — MERGED 2026-11-12 via PR #152 (`1676ab0c`):
  `RouteInfo` matches the runtime (one path parameter per placeholder of the
  full template, 200 as the default status for every verb). Generated specs
  change for a `POST`/`DELETE` that relied on the old 201/204 default. Closes
  with the next `r2e-macros` release.
- **#1245 (medium)** — in-process MCP test client over the `McpLoopback`
  bean, the only survivor of the dropped W6. **Implemented** on
  `task/1245-mcp-test-client` (in review): `app.mcp()` lives in
  `r2e_mcp::testing` (`McpTestExt` on `TestApp`; `r2e-test` does not depend on
  `r2e-mcp`), `.as_user()` mints a `TestJwt` bearer bound to the endpoint's
  resource, notifications are recorded. Additive — closes with the next
  `r2e-mcp` release.
- **#1181 follow-ups** (ticket closed, released in `r2e-macros` 0.11.0):
  close-the-loop still has to replace its `schedule_task_with` workaround in
  `ctl-api/src/controllers/sweep.rs` (app side). Not V1: live cadence
  re-evaluation per re-arm (driver change), `Option<ScheduleConfig>` → `None`
  disables the job.
- **#768 (low, idea)** — r2e-observability sqlx spans + app span helpers;
  measure the appetite first.
- `docs/research/thread-per-core.md` — stall detector, `#[offload]`/
  `#[blocking]`, `threads_per_worker` (none implemented).
- `plans/transport-layering.md` — share more between `#[routes]` /
  `#[grpc_routes]` (and the former `#[mcp_routes]`). **Levels 1 + 2 + 3a'
  RELEASED 2026-11-04 as r2e 0.11.0** (#1212 PR #132, #1213 PR #133, #1215
  PR #136, #1214 banner formatter; release PR #138, merge `2294b763`, tags
  `r2e-v0.11.0` / `r2e-mcp-v0.11.0` / `r2e-grpc-v0.10.0` /
  `r2e-security-v0.10.0` / `r2e-macros-v0.10.0`, …). Doctrine since
  2026-11-04: **one type = one port** (plan §5) — the mixed HTTP + MCP form
  never shipped, `#[mcp_routes]` was removed outright (no alias), 3b
  (neutral signature) and 3c (single registration) are dropped. The release
  carries every user-facing break of the workstream; what follows is
  non-breaking by design (plan §8), each ticket on its own branch from
  master, nothing started as of 2026-11-04:
  - **#1218 Hexagonal DX 1** (small, no dependency, **start here**):
    `From<HttpError> for tonic::Status` in r2e-grpc with the
    `rejection_to_status` policy, so a `#[derive(ApiError)]` domain error is
    consumed as is by an HTTP handler, a `#[tool]` and a `#[grpc_routes]`
    method; one-line notes in `llm/error-handling.md` + `llm/grpc.md`.
  - **#1216 3b'** (large, no dependency): one generation pipeline inside
    r2e-macros over the shared `Vec<MemberDef>` (port description + three
    implementations HTTP/MCP/gRPC); gRPC joins it keeping `#[grpc_routes]`
    (user methods inside a generated trait impl carrying the assoc types, so
    `substitute_self_assoc` goes away); the three `*_routes_parsing.rs` lose
    their member loops. Oracle: `cargo expand` byte-identical on example-app /
    example-mcp / example-grpc. Patch of r2e-macros, no `Breaking` entry.
  - **#1217 3c'** (low, five independent items = five commits; ticket
    refreshed 2026-11-12): gRPC core store through `endpoint_core_or_insert`
    (only r2e-core and r2e-mcp call it today); compile-time rejection of
    `register_controller` on the pre-state builder (silently discarded at
    `build_state()` today); hidden deco item names `<Ctrl>_<fn>` colliding
    across modules; `Self` unscoped in HTTP deco expressions; `GrpcIdentity`
    stub removal — a `Breaking` entry of r2e-grpc. The stub survived
    r2e-grpc 0.11.0, so it goes with the next breaking minor of r2e-grpc.
    The former `www-authenticate` item shipped under #1173.
  - **#1219 Hexagonal DX 2** (medium, after #1218): example-app as
    domain/ports/adapters{http,mcp,grpc,events}/services, outbound ports as
    `Arc<dyn Port>` beans swapped with `override_bean` in a test,
    `llm/hexagonal.md` (routed from `llm.txt`, slug in `TOPICS`, doctested),
    book chapter, inbound adapter catalogue (what each adapter injects and
    projects, incl. the one-port-per-type compile error). No DB queue/outbox.
  - **#1220 Hexagonal DX 3** (small, after #1219): `r2e doctor` rule against
    transport types leaking into `domain/` and `services/` (heuristic on
    `r2e::http` / `Json` / `ToolCall` / `tonic` imports).
  Recommended order: #1218 → #1216 (with #1217 items landing in parallel as
  single commits) → #1219 → #1220.

---

## Decisions log — do NOT re-propose

- **W6 testing-DX follow-ups: DROPPED (2026-11-12, user decision).** No dev
  services for Kafka / RabbitMQ / Pulsar (the generic `DevService` covers
  them), no `r2e doctor` check for missing dev-service config, no
  `r2e test --watch` (Phase 3, never approved). The shipped part is in the
  archive § W6; the in-process MCP test client lives on as Tasker #1245.
- **No closure-based config path on the builder (2026-11-09, #1288).**
  `plugin_with(|cfg| plugin)`, `apply(|b| ..)`-for-config or any
  "read `r2e_config()` mid-chain to construct a plugin" API: REJECTED. A
  plugin whose behaviour depends on config declares `type Config` +
  `CONFIG_PREFIX` and merges builder field > file > default in `build`, where
  the section arrives already loaded — the contract that keeps `.plugin()`
  order irrelevant. `r2e_config()` is a non-DI read (tests, `main`) only.
  #1288 put the two stragglers (`Cors` → `cors.*`, `OpenApiPlugin` →
  `openapi.*`) on the contract and rewrote the docs that taught the chain
  split. A list value from a single `${VAR}` (comma split) is a separate
  ticket if ever wanted; today `${VAR}` is one entry.
- **`#[scheduled(config = "…", default = "…")]`: REJECTED (2026-11-06,
  user decision, #1181).** It would be a second config-reading path next to
  `#[config]` (own default handling, own boot validation, own `config_keys()`
  declaration for the dev-reload fingerprint) and only covers "cadence = one
  key". The shipped form is `#[scheduled(schedule = "method")]` — a sync
  `&self -> ScheduleConfig` method on the same impl block, evaluated once at
  registration — which reads the cadence from a `#[config]` field, an
  injected bean, or code. Defaults live in YAML (`${VAR:1h}`) or in the method
  (`Option<ScheduleConfig>`). Do not re-propose a config key on the attribute.
- **One type = one port (2026-11-04).** Inbound ports (HTTP, MCP, gRPC,
  consumers) are separate `#[controller]` types over the same beans
  (hexagonal). The 3a mixed HTTP + MCP impl was removed before release; a
  `#[tool]` next to a `#[get]` is a compile error. Do not re-propose
  multi-port types, a neutral member signature (3b) or a single
  `register_endpoint` (3c). "Unify" means the generator code in r2e-macros.
- **Qualifiers / named beans: REJECTED.** Newtypes are the chosen pattern for
  same-typed beans (runtime `DuplicateBean` backstop) (the `name = "…"`
  shorthand was removed 2026-08-28).
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
- **Tracing install: no `tracing_subscriber::reload`, no thread-local
  `set_default` guard held across awaits** (rejected 2026-10-31). The
  subscriber is installed once by `build_state()`; pre-install logs go through
  the boot-log buffer (`capture_boot_logs`). No process-global extension
  registries either (late layers, noise rules, enrichers, console factory):
  everything is a layer contributed before the install.
- **Dev-reload re-reads `application.yaml` per patch** (deliberate: config is
  not pinned across hot-patches, unlike `.provide()`-ed beans).
- **One guard trait for every wire** (superseded 2026-10-09 by #1072 P3): the
  former "per-transport guards until a third wire" rule and its
  `GrpcRolesGuard`≈`RolesGuard` duplication are gone — gRPC and MCP build a
  `GuardContext` and project the `Rejection` by kind. Do not reintroduce a
  transport-specific guard trait.
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
