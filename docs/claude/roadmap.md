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
framework at the same seams — those seams are the Tasker #635 gap tasks (W2).

---

## W2 — Framework gaps found in real apps → tracked in Tasker #635

Moved to Tasker: umbrella task **#635 "R2E framework gaps from real-app audit
(threaty + patina)"** (target `r2e`), one sub-task per gap: EventBus↔SSE
bridge, proxy/streaming catch-all path, dynamic scheduled tasks, first-class
multipart, config-derive expressiveness, serve lifecycle / awaited drain,
auth-required without a phantom identity field, AI-facing DX. Full evidence
per gap in the Tasker sub-tasks and in this file's git history (`6d880f6`).

## Plugin DX/DI overhaul (PR #29) — SHIPPED (phases 1–6)

Landed in six phases: typed `Provided`/`Deps`, `LateDeps` + post-state
`configure`, typed plugin `Config`/`CONFIG_PREFIX` with boot validation,
provided-bean lifecycle hooks, conditional plugins + module-declared required
plugins. Authoritative reference: `docs/claude/plugins.md`; phase detail in
this file's git history.

**Leftovers folded back into this backlog:**

- **Type-level `Deps`-hole fix (deferred from phase 3)** — a `.register()`-ed
  type in a plugin's `Deps` still panics at runtime (steering to `LateDeps`)
  rather than failing at compile time; tagging `P` was judged to churn
  `Contains`/`AllSatisfied` everywhere. Revisit only if it bites in practice.
- **Serve-path e2e per plugin** — still open, see **W4** below (phase-6's
  `enabled` gate widens the surface: a disabled plugin's serve promise must
  also be verified as *absent*).
- **Full disposal semantics** — phase 5 shipped opt-in `run_pre_destroy`; the
  general `@PreDestroy`/drain-ordering story stays under **W5**.

## W4 — Plugin serve-path e2e audit

The item-12 failure mode (gRPC `serve()` silently unwired) generalized:
verify every plugin's serve-time promise through the real
`build_state → serve()` path — prometheus, observability, oidc, openapi,
static, scheduler, health. One e2e test per plugin, in the spirit of
`example-grpc/tests/grpc_serve.rs`.

## W5 — Carried-over DI items — DONE (2026-07-17)

- **Controller-level interceptor instance sharing — SHIPPED.** Impl-level
  `#[intercept]` now builds one instance per controller per dispatch surface
  (one shared across all HTTP routes via a hidden per-controller deco set;
  one shared across scheduled/consumer via the core's deco slot) instead of
  one per route/method. Route-level stays per-route; gRPC and `#[bean]`
  impl-level stay per-method (deliberate). Reference:
  guards-interceptors.md; sharing proven by
  `examples/example-app/tests/controller_intercept_sharing_test.rs`.
- **Compile-time scalability — MEASURED, no action needed.** threaty
  (20 controllers, 136 injections, `recursion_limit = 256`): full
  registration-crate recompile ~10 s, incremental ~2.4 s. HList cost shows
  in typeck (~1.4 s) + part of the monomorphization walk; the dominant cost
  is ordinary codegen/LLVM of handler code. Quadratic only becomes the
  bottleneck at much larger registration counts — revisit then. Dep lists
  still not bean-deduped (same trigger).
- **Bean disposal hooks — SHIPPED.** `#[pre_destroy]` on `#[bean]` methods
  and `#[routes]` controller impls, mirroring `#[post_construct]` (same
  signature/rejection matrix + `lazy` conflict). Runs in the async graceful-
  shutdown phase: controller hooks first, then bean hooks, reverse
  registration order; `Err` logged and swallowed; pinned `override_bean`
  skips it. Does NOT fire on `build_with_consumers`/`TestApp` (no shutdown).
  Docs: beans-di.md, subsystems.md, CLAUDE.md, llm.txt.

## W6 — Testing DX follow-ups

- Dev services for the remaining backends: Kafka, RabbitMQ, Pulsar
  (crate `r2e-devservices`, same workspace-session/Ryuk lifecycle).
  **OpenFGA: DONE (2026-07-18)** — `DevOpenFga` (feature `openfga`,
  `r2e-devservices/src/openfga.rs`: shared container + Ryuk, HTTP bootstrap
  helpers `create_store`/`write_model`/`write_tuples`, IDs injected via
  `override_config_value`) + dedicated `examples/example-openfga` (canonical
  `impl App` shape, `FgaCheck` guards, DevOpenFga-backed tests). Runtime-
  verified against the live container (2026-07-18): dev-service smoke test
  + all 4 example-openfga integration tests green. Note: `-- --ignored`
  also makes rustdoc RUN `ignore`d doctests — target the integration file
  (`--test openfga_test -- --ignored`) when re-running.
- Demo dev-services usage in `example-postgres`.
- `r2e doctor` check for missing dev-service config (deliberately NOT
  auto-sniffing config — implicitness hides failures).
- **Phase 3 (`r2e test --watch`): deferred, NOT approved** — do not start
  without an explicit user go.

## W7 — Docs / CLI alignment pass — DONE (2026-07-17)

CLI templates turned out already current (every canonical idiom present, no
stale APIs). The book was the real gap: `testing/integration-patterns.md`
taught the shared-`setup()`/`from_builder` anti-pattern (rewritten to
`#[r2e::test(app = ...)]` + `.as_user()`), test-jwt/test-app/test-session
pages moved to app-boot idioms, observability page's false "auto tracing
init" claim corrected, embedded-oidc comments modernized. `#[r2e::main]`
still exists (runtime-builder options only — the `main(env)`-param form is
gone); reference-level docs of current low-level APIs (`from_builder`, raw
`jwt.token()`) were deliberately kept. The AI-facing-DX lever (Tasker #635)
remains open as its own sub-task.

## W8 — EventBus perf & reliability — SHIPPED (PR #30)

P1–P5 fixed across the local bus + the four distributed backends (delivery
semantics, reconnect bugs, batching/pipelining, micro-opts); only P4.4
deferred. Breaking `request`/`respond` API change. Full audit, plan and
file:line evidence: `docs/claude/eventbus-perf.md`.

## W9 — `App` trait canonicalization (Tasker #667) — follow-ups

The single canonical app-declaration landed: `impl App for MyApp` (`setup`/`build`)
launched by `r2e::app_main!(MyApp)` (and `launch!` for custom entrypoints), replacing the inline-main / blueprint-fn /
`app_with_env` / `#[r2e::main]`-with-param zoo; `with_config` → `override_config`
(test-harness in-memory stash — no longer dev-reload plumbing; `build` re-runs
per patch and re-reads `application.yaml` from disk). Docs, `llm.txt`, and CLI
scaffolding are aligned.
**Examples canonicalization — DONE (2026-07-17):** all eight remaining
examples migrated to the `app.rs` (`impl App`) + `lib.rs` + thin
`app_main!`/`launch!` main.rs shape (executor, postgres, multi-tenant,
websocket-chat, grpc, sharded-bench, oidc restructured; microservice
converted from two `#[path]`-include bins to a real lib with
`ProductApp`/`OrderApp` + explicit `with_config_file` per service). All
examples gained the `dev-reload` passthrough feature. example-grpc's
transport-level tests deliberately stay on their dedicated harness
(`TestApp::boot` doesn't cover the separate gRPC port).

Remaining:
- Phase 2: pin previous `BeanContext` instances across hot-patches so **all**
  bean state survives (not just `Env`) — validate Subsecond vtable semantics
  before relying on it (needs live hot-reload validation; not agent-friendly).

## W10 — Bean/controller feature unification — DONE (phases 1–4 + follow-up, 2026-07-16/17)

The controller core IS a bean: transverse member attributes (`#[scheduled]`,
`#[consumer]`, `#[intercept]`, `#[async_exec]`, `#[post_construct]`) are
implemented once at the bean level (`r2e-macros/src/codegen/transverse.rs`)
and shared by `#[controller]`/`#[routes]`, which only add the transport layer.
`#[scheduled]`/`#[consumer]` beans auto-collect at `build_state()`;
`#[transactional]` and `AppBuilder::register_subscriber` are REMOVED
(BREAKING; see decisions log). No dedicated `#[service]` macro — unification
beats a third shape; `#[derive(BackgroundService)]` stays the escape hatch.
Full phase-by-phase record (design decisions, test/fixture lists, post-review
fixes) in this file's git history (pre-2026-07-17 versions); authoritative
docs: beans-di.md, guards-interceptors.md, executor.md, subsystems.md,
llm.txt.

## W11 — Items carried from the root `todo` file (triage 2026-07-17)

Triage of the root scratch `todo`: everything else in it had already shipped
(App trait, SSE broadcaster, ExceptionMapper → `#[derive(ApiError)]`,
structured validation errors, config validation, testing DX, EventBus
backends, scheduler rework, W10 unification, JSON tracing, axum confinement).
Still open:

- **OpenFGA path-param compile check — DONE (2026-07-18).** Literal
  `FgaCheck...from_path("name")` references in `#[guard]` exprs are now
  compile-checked against the route's `{param}` placeholders (method path +
  controller `path = "..."` prefix, via a spanned `const _` assertion —
  `r2e-macros/src/codegen/mod.rs`). Dynamic/non-literal forms fall through to
  the runtime backstop (kept). Trybuild pass+fail fixtures added
  (`r2e-compile-tests/compile-{pass,fail}/openfga_from_path_*`).
- **OpenAPI: warn on unmappable response body — DONE (2026-07-18).** The
  macro records the unmappable return type (`RouteInfo.response_unmapped`,
  BREAKING field addition) and `build_spec` emits one boot-time
  `tracing::warn!` per gap (route, type, opt-out hint) — also covers
  schema-less `Json<T>` request/response bodies. Testable seam:
  `r2e_openapi::spec_warnings()`. Docs: llm.txt + subsystems.md.
- **gRPC/proto automagic setup** — auto build.rs / proto compilation
  scaffolding (`r2e add grpc`-grade DX: drop a `.proto`, get a compiled
  service). Related tech-debt note below (trybuild fixture hand-fakes tonic).
- **Zero-copy exploration (xitca-web)** — exploratory only: evaluate whether
  a zero-copy HTTP layer brings measurable wins over the current axum stack.
  No commitment.
- **Responsibility-boundaries audit (remainder)** — the scheduled/consumer
  half was absorbed by W10; what remains is a pass over which concern lives
  in which crate/macro (core vs http vs macros vs integrations).

## Tech debt (deferred, low priority)

- **Event bus perf** (2026-03 audit): superseded by W8 — the two still-
  deferred items (`Arc<EventMetadata>`, lazy `EventMetadata::new()`) are
  carried in `eventbus-perf.md` § Explicitly deferred.
- **gRPC trybuild fixture** hand-fakes the tonic server surface (no
  proto/build.rs) — drift risk on tonic bumps.

---

## Decisions log — do NOT re-propose

- **Qualifiers / named beans: REJECTED.** Newtypes are the chosen pattern for
  same-typed beans (runtime `DuplicateBean` backstop).
- **`#[transactional]`: REMOVED (W10 phase 4, 2026-07-16, user-approved).**
  `#[managed]` is the single transaction story. The body wrapper had zero
  usage, relied on an unhygienic magic `tx` variable injected into the body
  scope, and every doc already said "prefer `#[managed]`". Do not reintroduce
  it — extend `ManagedResource` instead if a gap shows up.
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
