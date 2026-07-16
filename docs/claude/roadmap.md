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

## W1 — Migrate threaty to R2E master → owned elsewhere

Handled by the maintainer in a separate work stream (Tasker task, target
`threaty`) — NOT part of this backlog; do not pick it up from here. Migration
frictions it surfaces still land as r2e sub-tasks under Tasker #635, and the
post-migration build serves as the compile-time-scalability data point (W5).

## W2 — Framework gaps found in real apps → tracked in Tasker #635

Moved to Tasker: umbrella task **#635 "R2E framework gaps from real-app audit
(threaty + patina)"** (target `r2e`), one sub-task per gap: EventBus↔SSE
bridge, proxy/streaming catch-all path, dynamic scheduled tasks, first-class
multipart, config-derive expressiveness, serve lifecycle / awaited drain,
auth-required without a phantom identity field, AI-facing DX. Full evidence
per gap in the Tasker sub-tasks and in this file's git history (`6d880f6`).

## W3 — Migrate patina (escape-hatch hardening)

Small API surface (2 controllers, 6 injects) but it exercises exactly the
seams of the Tasker #635 gaps (proxy/streaming, dynamic scheduled tasks,
config derive) plus `TestApp::from_builder` → blueprint boot, and
testcontainers-Postgres-by-hand → `DevPostgres`. Do after (or interleaved
with) the corresponding gap tasks so the migration lands on supported API
instead of re-pinning to internals.

## Plugin DX/DI overhaul (PR #29) — SHIPPED (phases 1–6)

The plugin system rework landed in six phases (authoritative reference:
`docs/claude/plugins.md`). Shipped:

- **1–2** — `PreStatePlugin` simplified surface (typed `Provided`/`Deps`, no
  builder generics), tuple `Provided` for multi-bean plugins.
- **3** — `LateDeps` + post-state `configure` (resolves factory-built and
  other-plugin beans after `build_state()`).
- **4** — typed `Config` / `CONFIG_PREFIX`, boot-time section validation,
  builder > file > default precedence (Prometheus reference).
- **5** — provided-bean lifecycle hooks (`run_post_construct` / `run_pre_destroy`).
- **6** — conditional plugins (`<prefix>.enabled`, config-driven, beans always
  survive) + module-declared required plugins (`#[module(requires_plugins(..))]`
  / `FeatureModule::RequiredPlugins`, plugin-named compile diagnostic).

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

## W5 — Carried-over DI items (opportunistic)

From the completed DI backlog (details in git history of `di-next-steps.md`):

- **Controller-level interceptor instance sharing** — controller-level
  `#[intercept]` builds one instance per route; could share one per
  controller. Only worth it for stateful interceptors.
- **Compile-time scalability watch** — the HList machinery is O(n²)-ish in
  registrations; measure on a real app (threaty post-migration = the perfect
  data point — the Tasker threaty-migration task feeds this). Dep lists are
  not bean-deduped (revisit only if
  build times hurt). `recursion_limit = "512"` needed past ~127 registrations.
- **Bean disposal hooks** — `@PreDestroy` equivalent; becomes concrete once a
  real DB app runs on master (threaty again).

## W6 — Testing DX follow-ups

- Dev services for the remaining backends: Kafka, RabbitMQ, Pulsar, OpenFGA
  (crate `r2e-devservices`, same workspace-session/Ryuk lifecycle).
- Demo dev-services usage in `example-postgres`.
- `r2e doctor` check for missing dev-service config (deliberately NOT
  auto-sniffing config — implicitness hides failures).
- **Phase 3 (`r2e test --watch`): deferred, NOT approved** — do not start
  without an explicit user go.

## W7 — Docs / CLI alignment pass

CLI templates (`r2e new`, `r2e add`, `r2e generate`) and the book still
reflect pre-refactor idioms in places; align them with blueprint boot, HList
state, `.register()`, `DecoratorSpec` guards, and pinned test overrides.
This is also the main lever for the AI-facing-DX gap (Tasker #635).

## W8 — EventBus perf & reliability (hub: `eventbus-perf.md`) — SHIPPED (PR #30)

Full 2026-07-12 audit of LocalEventBus + the four distributed backends
(iggy/kafka/pulsar/rabbitmq) found: local bus and shared `BackendState`
sound; distributed backends not production-grade (per-emit round-trip with no
batching, ack/commit before handler = silent at-most-once, RabbitMQ reconnect
broken, Pulsar global producer lock, cross-process event_id dedup collision).
Fixed across P1–P5 (P1 semantics → P2 bugs → P3/P4 throughput → P5 micro-opts);
only P4.4 deferred. Note the breaking `request`/`respond` API change. Plan and
file:line evidence live in `docs/claude/eventbus-perf.md`.

## W9 — `App` trait canonicalization (Tasker #667) — follow-ups

The single canonical app-declaration landed: `impl App for MyApp` (`setup`/`build`)
launched by `r2e::app_main!(MyApp)` (and `launch!` for custom entrypoints), replacing the inline-main / blueprint-fn /
`app_with_env` / `#[r2e::main]`-with-param zoo; `with_config` → `override_config`
(test-harness in-memory stash — no longer dev-reload plumbing; `build` re-runs
per patch and re-reads `application.yaml` from disk). Docs, `llm.txt`, and CLI
scaffolding are aligned.
Remaining:
- Canonicalize the remaining examples (microservice, postgres, …) to the `App`
  trait (example-app already migrated).
- Phase 2: pin previous `BeanContext` instances across hot-patches so **all**
  bean state survives (not just `Env`) — validate Subsecond vtable semantics
  before relying on it.

## W10 — Bean/controller feature unification (in progress — phases 1–3 shipped 2026-07-16)

Evidence: feature-matrix audit (2026-07-16). Transverse concerns are
controller-only by implementation accident, not by design — `#[scheduled]`,
`#[async_exec]`, `#[transactional]`, and `#[intercept]` only exist because the
machinery (DecoSlot, wrapping, registration-time collection) was built inside
`#[routes]`; `#[consumer]` exists on both; `#[post_construct]` is bean-only.
Symptom: beans-di.md's own "When to use" table recommends `#[scheduled]` for
periodic tasks, which does not work on a bean. Absorbs the todo items
"macro de service vs uniquement controller" and the scheduled/consumer half of
"audit de responsabilities boundaries".

**Target**: the controller core IS a bean. Transverse member attributes are
implemented once at the bean level; `#[controller]`/`#[routes]` only add the
transport layer (routes, request façade, guards/roles, OpenAPI). A controller
may still carry `#[scheduled]`/`#[consumer]` — not as controller features but
because a controller is a bean.

Phases (quality-review gate after each, same convention as the controller
refactor):

1. **`#[scheduled]` on `#[bean]` — DONE (2026-07-16).** `ScheduledSource`
   trait in `r2e-core/src/scheduled_source.rs` (signature takes
   `&BeanContext` so phase 3 can delegate the controller path to it);
   `#[bean]` scans `#[scheduled]` methods (shared parser/codegen with
   controllers: `extract/scheduled.rs`, `codegen/scheduled.rs`) and emits the
   impl + an `after_register` hook. **Registration is auto-collection at
   `build_state()`** (user-approved; NOT an explicit `register_scheduled`
   call): `BeanRegistry::register_scheduled_source` queues a hook, drained by
   `build_state()` on the typed builder (after deferred plugin actions, so
   the Scheduler's `TaskRegistryHandle` exists) into the same
   `ScheduledTaskMarker` pipeline. Hooks read the bean by type from the
   resolved graph → pinned test overrides are honoured (post-construct
   semantics). `#[intercept]` on bean scheduled methods is an explicit
   compile error (divergence until phase 2), as are `lazy` + scheduled and
   scheduled + consumer on one method. Tests:
   `examples/example-app/tests/bean_scheduled_test.rs`, compile-pass/fail in
   `r2e-compile-tests` (`bean_scheduled*`). Docs: beans-di.md, subsystems.md,
   llm.txt.
2. **Bean-level decorators — DONE (2026-07-16).** `#[intercept]` on bean
   `#[scheduled]`/`#[consumer]` methods + impl-level `#[intercept]` on the
   `#[bean]` impl, via the existing `DecoratorSpec`/`build_decorator` +
   `generate_named_deco_items`/`wrap_with_deco_interceptors` machinery; spec
   `Deps` folded into `Registrable::Deps` (`deps_fold_from_base`), so a missing
   bean is a compile error at `.register` (runtime `dependencies()` stays
   constructor-only). Per-instance storage via a new `SharedDecoSlot`
   (Arc-shared across clones, unlike controller `DecoSlot` which clones empty —
   beans are cloned by value before the fill) + `HasDecoSlot` trait
   (`on_unimplemented` names the fix). `#[bean]` now also works **on the
   struct** (injects the hidden `pub #[doc(hidden)] __r2e_decos` field + slot
   impl); the impl macro rewrites in-block struct literals to init it (skips
   `..rest`). Single fill at `build_state()` before `#[post_construct]`
   (`BeanRegistry::register_deco_fill` + `BeanDecoFill`). Direct calls
   self-intercept; sync scheduled sources promoted to `async fn`; consumer
   interception covers subscribers AND responders (bean `#[consumer]` gained
   responder support). Rejected: `#[intercept]` on a plain bean method.
   Tests: `examples/example-app/tests/bean_intercept_test.rs`, compile
   pass/fail `bean_intercept*` in `r2e-compile-tests`. Docs: beans-di.md,
   guards-interceptors.md, llm.txt.
3. **Controller core = bean — DONE (2026-07-16).** `#[routes]`' transverse
   codegen delegates to the shared emitters in
   `r2e-macros/src/codegen/transverse.rs` (extracted from `bean_attr.rs` in a
   first byte-identical-output step, verified via `cargo expand` diff).
   Deleted: `controller_impl.rs::generate_scheduled_tasks`,
   `generate_consumer_registrations`, `wrapping.rs::generate_scheduled_method`
   (~380 lines of duplication). The generated
   `Controller::scheduled_tasks_boxed`/`register_consumers` overrides embed
   the shared bodies cloning the `Arc<core>` — a direct
   `impl ScheduledSource/EventSubscriber for Arc<Name>` in the user crate is
   an orphan-rule violation (E0117), so the emitters expose the bodies
   parameterized by an instance expression; beans still get the trait impls.
   Deco container + `impl BeanDecoFill for Name` fills the existing `DecoSlot`
   (slot types stay distinct per the phase-2 decision; fill is now an explicit
   registration step via the new default-no-op `Controller::fill_decos`,
   called once right after `construct` — required because
   intercepted-consumers-only controllers need the fill too; the extra fill
   inside `scheduled_tasks_boxed` is OnceLock-idempotent, kept for the manual
   test path). **New:** `#[intercept]` on controller `#[consumer]` methods
   (method + impl level, impl outermost, responders included, direct calls
   intercepted; spec deps folded into `controller_deps_fold` → missing bean is
   a compile error at `.register_controller`); `#[post_construct]` on
   controllers (queued at registration via the new `Controller::post_construct`
   hook + `post_construct_registrations` builder vec, runs at startup BEFORE
   consumer registrations at both boot paths — `prepared.rs` serve propagates
   Err, `build_with_consumers` panics; plain `build()` drops both, documented).
   `BeanDecoFill`/`PostConstruct` lost their `Clone` supertrait (Clone moved
   to the bean-registration call sites) so non-Clone controller cores can
   implement them. New compile errors on controllers (previously silent):
   `#[scheduled]`+`#[consumer]` on one method, stray `#[intercept]` on a plain
   method, invalid `#[post_construct]` placements. Tests:
   `examples/example-app/tests/controller_transverse_test.rs`, compile
   pass/fail `controller_transverse*`, `controller_scheduled_consumer_conflict`,
   `controller_post_construct_*`, `controller_stray_intercept`,
   `consumer_intercept_missing_dep`. Docs: CLAUDE.md, beans-di.md,
   guards-interceptors.md, subsystems.md, llm.txt. Review gate:
   pass-with-nits, all nits applied.
4. **Relocate `#[async_exec]`** (and evaluate `#[transactional]`) to the bean
   level; decide whether controller-only placement is kept or deprecated.

Phase-1 design decisions (settled): registration is **auto-collection at
`build_state()`** — user-approved 2026-07-16; matches controller
auto-discovery and avoids the silent no-op of a forgotten explicit call
(follow-up idea, not scheduled: align `#[consumer]` beans on the same
auto-collection and retire `register_subscriber`). No dedicated `#[service]`
macro — unification beats a third shape; `#[derive(BackgroundService)]` stays
the escape hatch for hand-written loops.

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
