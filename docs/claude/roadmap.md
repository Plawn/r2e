# R2E Roadmap

Status: **LIVE WORKING BACKLOG** — created 2026-07-10, after the DI/builder
refactor (phases 1–6 + backlog, hub: `di-builder-refactor.md`) and the
testing-DX rework (phases 1+2) both landed on master. This file replaces the
completed plan docs (`plan-controllers-as-beans.md`, `plan-feature-modules.md`,
`plan-guards-as-beans.md`, `plan-testing-dx.md`, `di-next-steps.md`) and the
root-level `plan.md` / audit reports — their shipped content lives in the
reference docs and git history; only still-open work is carried here.

## North star

R2E must **compound like Quarkus**: every feature plugs into DI, config,
testing, OpenAPI and observability with zero liaison code. Optimized for
humans **and** AI agents writing clean, well-architected, fast apps — the
idiomatic R2E path must always be the shortest, most discoverable path;
whenever a real app drops to raw axum or hand-rolls infrastructure, that is a
framework bug to record here.

## Evidence base: the 2026-07-10 real-app audit

Two production-bound apps built on pre-refactor R2E (~pin `v0.2.56` /
`0397c4b`, 66–67 commits behind master) were audited as framework probes:

- **threaty** (`~/Documents/threaty`, ~44K LOC, 4 crates) — deep user:
  20 controllers, ~101 routes + 6 SSE + 3 scheduled, 138 injections,
  48 path-parameterized guards, internal+external OIDC, 3 custom
  `PreStatePlugin`s.
- **patina** (`~/Documents/blumana/patina`, ~23K LOC, 9 crates) — shallow
  user: 0 provided beans, hand-built 10-field state, 2 controllers; the
  entire registry-proxy core is a hand-written axum fallback handler.

Verdict: where the apps follow the R2E path, compounding works (config, DI,
scheduler, metrics, health, OIDC come nearly free). Both apps leak out of the
framework at the same seams — those seams are workstream W2 below.

---

## W1 — Migrate threaty to R2E master (the validation vehicle)

The single highest-value next step. Threaty exercises DI, guards, SSE, OIDC,
plugins and the new test harness at once; every migration friction is a
framework ticket to add here. Breaking changes it must absorb (pin → master):

- `#[derive(Controller)]` + `#[controller(state = AppState)]` → attribute-only
  `#[controller]`, no `state` key (controllers are state-generic).
- Hand-written `AppState` (41 fields, `#[derive(BeanState)]` — deleted) +
  42 `.with_bean` + `build_state::<AppState,_,_>()` turbofish →
  `.register::<T>()` / `.provide(...)` + `build_state()` (HList state).
- `Guard<AppState, AppUser>` (its 5 macro-generated resource guards) →
  `Guard<I>` + `DecoratorSpec` (guards built once at registration; verify
  typed path symbols `path::pid` survive per-request via `GuardContext`).
- `cache_backend()` global (used in `sync_chain_service.rs`) → cache store
  bean (`InMemoryStore::shared()`).
- `PreStatePlugin` ×3 + middleware `Plugin` ×2 → current plugin signatures
  (`RawPreStatePlugin::install` gained `Mods`); the two hand-written frontend
  plugins should become `r2e-static`.
- Major dep bumps ride along (sqlx 0.9, tonic 0.14, lapin 4…).
- Adopt the blueprint pattern (lib `app(b) -> impl BootableApp` + thin bin)
  and write its **first HTTP tests** via `#[r2e::test(app = ...)]` +
  `DevPostgres` — 44K LOC currently has zero.

## W2 — Framework gaps found in real apps (prioritized)

1. **Real-time fan-out / EventBus↔SSE bridge.** Threaty enables the `events`
   feature but never uses the EventBus: it wrote its own broadcaster macro
   (newtype + `#[bean]` + `Deref` over `SseBroadcaster`) and fans out via SSE
   by hand (`threaty-domain/src/broadcasters.rs`). Wanted: first-class
   broadcast/topic beans and a zero-liaison bridge (e.g. a `#[consumer]` that
   feeds an SSE stream).
2. **Proxy/streaming path.** Patina bypasses R2E for 100% of its business
   traffic: fallback dispatch by content-type, streamed responses
   (`handler.rs`), per-protocol auth. Wanted: catch-all/wildcard routes as
   first-class controller routes, or at minimum a stable, documented
   escape-hatch pattern (today: `with_layer_fn` + `r.fallback(...)`).
3. **Dynamic scheduled tasks.** Patina registers config-driven task sets by
   reaching into internals (`get_plugin_data::<TaskRegistryHandle>` +
   `ScheduledTaskMarker` + boxed `ScheduledTaskDef`s). `#[scheduled]` only
   covers static tasks. Wanted: a public dynamic task-registration API.
4. **First-class multipart.** Threaty imports `axum::extract::Multipart`
   directly (3 sites). Wanted: an R2E-owned extractor re-export + OpenAPI
   modeling (r2e-test already ships multipart upload builders).
5. **Config derive expressiveness.** Patina hand-wrote 2 `impl
   ConfigProperties` touching internal API (`TNil`, `PropertyMeta`) —
   presumably for dynamic/map-shaped sections. Wanted: close that gap in the
   derive (map-valued sections, tagged enums), or expose a supported
   manual-impl surface.
6. **Serve lifecycle / graceful drain.** Patina hand-rolled connection
   draining (`begin_drain`/`wait_in_flight` + `DrainHealthIndicator`); this
   overlaps the known gRPC residual gaps: no programmatic `serve()`
   termination (tests abort the task), gRPC drain cancelled but not awaited
   at shutdown, `GrpcServer::with_reflection` unimplemented (warns loudly).
   Wanted: a real shutdown contract — awaited drain hooks + programmatic stop.
7. **Auth-required without a phantom field.** In threaty, most of the 82
   `#[inject(identity)]` fields exist only to force authentication (`_user`,
   never read). Wanted: a declarative "authenticated endpoint" marker with no
   dummy field.
8. **AI-facing DX.** Patina (largely AI-co-written) defaulted to axum idioms
   (custom `FromRequestParts` extractor instead of a guard, fallback instead
   of a controller). When the idiomatic path isn't the shortest visible one,
   humans and AIs route around the framework. Countermeasures: agent-facing
   docs, CLI scaffolds that bake in the blueprint pattern, "one obvious way"
   examples per subsystem.

## W3 — Migrate patina (escape-hatch hardening)

Small API surface (2 controllers, 6 injects) but it exercises exactly the
seams of W2 items 2/3/5 plus `TestApp::from_builder` → blueprint boot, and
testcontainers-Postgres-by-hand → `DevPostgres`. Do after (or interleaved
with) the corresponding W2 items so the migration lands on supported API
instead of re-pinning to internals.

## W4 — Plugin serve-path e2e audit

The item-12 failure mode (gRPC `serve()` silently unwired) generalized:
verify every plugin's serve-time promise through the real
`build_state → serve()` path — prometheus, observability, oidc, openapi,
static, scheduler, health. One e2e test per plugin, in the spirit of
`example-grpc/tests/grpc_serve.rs`.

## W5 — Carried-over DI items (opportunistic)

From the completed DI backlog (details in git history of `di-next-steps.md`):

- **Controller-level interceptor instance sharing** — controller-level
  `#[intercept]` builds one instance per route; could share one per
  controller. Only worth it for stateful interceptors.
- **Compile-time scalability watch** — the HList machinery is O(n²)-ish in
  registrations; measure on a real app (threaty post-migration = the perfect
  data point, W1 feeds this). Dep lists are not bean-deduped (revisit only if
  build times hurt). `recursion_limit = "512"` needed past ~127 registrations.
- **Bean disposal hooks** — `@PreDestroy` equivalent; becomes concrete once a
  real DB app runs on master (threaty again).

## W6 — Testing DX follow-ups

- Dev services for the remaining backends: Kafka, RabbitMQ, Pulsar, OpenFGA
  (crate `r2e-devservices`, same shared-per-process container pattern).
- Demo dev-services usage in `example-postgres`.
- `r2e doctor` check for missing dev-service config (deliberately NOT
  auto-sniffing config — implicitness hides failures).
- **Phase 3 (`r2e test --watch`): deferred, NOT approved** — do not start
  without an explicit user go.

## W7 — Docs / CLI alignment pass

CLI templates (`r2e new`, `r2e add`, `r2e generate`) and the book still
reflect pre-refactor idioms in places; align them with blueprint boot, HList
state, `.register()`, `DecoratorSpec` guards, and pinned test overrides.
This is also the main lever for W2 item 8 (AI-facing DX).

## Tech debt (deferred, low priority)

- **Event bus perf** (2026-03 audit, still deferred): `Arc<EventMetadata>` to
  avoid N clones per dispatch (revisit if headers/correlation_id get heavily
  populated with high fan-out); lazy `EventMetadata::new()` (revisit only if
  a zero-alloc local dispatch path becomes a goal).
- **gRPC trybuild fixture** hand-fakes the tonic server surface (no
  proto/build.rs) — drift risk on tonic bumps.

---

## Decisions log — do NOT re-propose

- **Qualifiers / named beans: REJECTED.** Newtypes are the chosen pattern for
  same-typed beans (runtime `DuplicateBean` backstop).
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
- **Per-transport guards until a third wire exists** (rule of three);
  `GrpcRolesGuard`≈`RolesGuard` ~30-line duplication accepted.
- **Dev services are explicit** (`DevPostgres::shared()`), never
  config-sniffed.
