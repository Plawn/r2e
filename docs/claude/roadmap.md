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
`PreStatePlugin`s) and **patina** (~23K LOC, shallow user: hand-built 10-field
state, registry-proxy core written as a raw axum fallback handler). Both leak
out of the framework at the same seams.

Tracked as umbrella task **#635 "R2E framework gaps from real-app audit
(threaty + patina)"** (target `r2e`), one sub-task per gap: EventBus↔SSE
bridge, proxy/streaming catch-all path, dynamic scheduled tasks, first-class
multipart, config-derive expressiveness, serve lifecycle / awaited drain,
auth-required without a phantom identity field, AI-facing DX. Full evidence
per gap in the Tasker sub-tasks and in this file's git history (`6d880f6`).
The **AI-facing-DX** sub-task is the one still clearly open.

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

## W12 — OpenFGA DX — Phase 4 (CLI), lowest priority

Phases 1–3 shipped 2026-07-20 (`.fga` parser + `model!` typed API, typed
`FgaClient` with write-through invalidation, `OpenFga` plugin owning the store
lifecycle at boot). Reference: `docs/features/23-openfga.md`.

Remaining: `r2e fga diff` / `push` / `pull` (diff local model vs store, pull an
existing store's model into a local `.fga`), plus tuple seed fixtures for
dev/tests. Nothing FGA-related exists in `r2e-cli` yet beyond the bundled doc.

## Open items tracked in their own docs

Kept where the context lives rather than duplicated here:

- `plans/phase1-optional-deps-conditional-beans.md` — example-app demo of the
  config-driven `#[producer] -> Option<T>` pattern.
- `plans/phase2-profiles-alternatives.md` — `#[bean(profile = "…")]` sugar
  (open design conflict with `P`), guaranteed profile groups, two profile test
  gaps (`R2E_PROFILE` precedence, `"default"` fallback).
- `docs/claude/controller-identity-codegen-refactor.md` — request-scoped
  helper methods (`#[request_helper]`), deliberately deferred.
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
