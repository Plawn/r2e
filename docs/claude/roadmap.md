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

**Phase 2 (bean pinning) — DONE (2026-07-19).** Dev-reload now does a
**partial rebuild** on fingerprint change: beans whose per-bean fingerprint
(constructor tokens + declared config values + transitive dep fingerprints)
is unchanged keep their instance across hot-patches — in-memory state
survives; changed beans and their transitive dependents rebuild (their
`#[post_construct]` re-runs, reused ones skip it). `.provide()`-ed values
are pinned from the previous cycle (except `R2eConfig` — YAML re-read per
patch stays deliberate); unchanged lazy slots carry over; eager/lazy mode is
part of the fingerprint; deco-fill targets and their transitive dependents
always rebuild (`DecoSlot` is a `OnceLock`, no in-place refill). Active-cycle
`#[pre_destroy]` hooks survive hot-patch future replacement. Two
pre-existing bugs fixed en route: the graph fingerprint now folds in the
**whole** config (`R2eConfig::full_fingerprint`) so an edit no bean declares
still refreshes the graph's `R2eConfig`; and the dev-reload caches +
lifecycle skip engage **only** under the real hot-patch loop
(`r2e::launch!` calls `mark_hot_reload_loop()`) — `cargo test
--features dev-reload` builds stay cold (the process-global caches used to
cross-contaminate builds in one test process). Core: `BeanRegistry::
resolve_reusing`/`ReusePlan` (beans.rs), CTX_CACHE (dev.rs), try_build_state
(builder/nostate.rs). Tests: `r2e-core/tests/dev_reload_partial.rs`
(4 cycles in-process). **Subsecond semantics validated live** (dx 0.7.3,
bin-only probe app, 2026-07-19): sibling-bean edit hot-patched with counter
state surviving, closures built by pre-patch code callable two patches deep,
inverse direction (stateful bean edited → rebuilt, sibling reused) OK.
Docs: 09-dev-mode.md, llm.txt.

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
- **gRPC/proto automagic setup — DONE (2026-07-18).** New `r2e-grpc-build`
  crate (`r2e-grpc/build/`): one-line build.rs (`r2e_grpc_build::compile()`)
  compiles every `proto/**/*.proto`, emits an aggregated per-package module +
  combined `FILE_DESCRIPTOR_SET` into OUT_DIR (rerun-if-changed — drop a
  `.proto`, get a compiled service); `r2e_grpc::include_protos!()` includes
  it. `r2e add grpc` is now a full scaffold (features + deps + build.rs +
  sample proto + `src/grpc.rs` skeleton); `r2e new --grpc` pre-wires a
  reflection-enabled service; `r2e generate grpc-service` updated.
  example-grpc migrated (dogfood). Also resolved the gRPC-trybuild tech-debt
  note: `r2e-compile-tests` now compiles `proto/ping.proto` via the helper
  and the 5 gRPC fixtures typecheck against real tonic output. Docs:
  17-grpc.md, cli.md, transport-adapters.md, llm.txt.
- **Zero-copy exploration (xitca-web)** — exploratory only: evaluate whether
  a zero-copy HTTP layer brings measurable wins over the current axum stack.
  No commitment.
- **Responsibility-boundaries audit (remainder)** — the scheduled/consumer
  half was absorbed by W10; what remains is a pass over which concern lives
  in which crate/macro (core vs http vs macros vs integrations).

## W12 — OpenFGA DX: schema-first, compile-time checked (proposed 2026-07-19)

**Goal:** the `.fga` authorization model becomes the single source of truth;
relations/types used in code are compile-checked against it, and the live
store is verified against it at boot. Closes the current gap where
`FgaCheck::relation("viewer").on("document")` is fully stringly-typed — a typo
or a relation absent from the model compiles fine and manifests as a permanent
(silent, fail-closed) 403 in prod.

**Current state (evidence):** only the path-param name is compile-checked
(`from_path(path::doc_id)`, W11 2026-07-18). The model itself is hand-written
JSON in the app (`examples/example-openfga/src/app.rs::document_model()`);
writes go through the raw tonic client + manual `registry.invalidate_object`;
nothing verifies that the compiled-in model matches what the store serves.

### Phase 1 — `model!` macro + generated typed API (the core) — SHIPPED 2026-07-20

Landed as specified below. Notes for later phases:
- Crates live at `r2e-openfga/model` (`r2e-openfga-model`) and
  `r2e-openfga/macros` (`r2e-openfga-macros`), following the `r2e-grpc/build`
  layout. The parser round-trips the entire vendored `openfga/language`
  transformer corpus (29 cases, `r2e-openfga/model/tests/corpus/`).
- Split discovered during implementation: the official transformer is
  **syntax-only** (its corpus contains semantically dangling refs), so the
  crate exposes `parse` (corpus-exact) + `validate` (semantic referential
  checks) separately; `model!` runs both.
- Parens + n-ary `or`/`and` are part of DSL 1.1 (not in the original plan's
  grammar sketch) — supported; operator *mixing* without parens is rejected.
- Generated subject markers for `DirectlyAssignable`: `user::Ty` (direct),
  `(team::Ty, team::Member)` (userset), `WildcardOf<user::Ty>` (`user:*`) —
  Phase 2 `grant`/`revoke` bounds consume these. `FgaSubject::subject_str()`
  renders the wire form.
- `example-openfga` migrated (guards use `authz::…`); its `document_model()`
  now derives from `authz::MODEL` — Phase 3 deletes it when the plugin owns
  apply/verify.
- Post-review hardening (code review 2026-07-20): condition bodies are
  captured **verbatim** (comment stripping is statement-only — a `#` inside a
  CEL string is not a comment); the injection guard covers `:`/`#`/`*` on
  BOTH sides (object ids in resolvers + `id()`/`try_id()`, and
  `identity.sub()` → 403 fail-closed before interpolating `user:{sub}`).
  Phase 2's `FgaClient` writes must apply the same subject/object character
  guards.

- New pure crate `r2e-openfga-model`: parser for the OpenFGA DSL 1.1
  (`.fga` → AST → schema-1.1 JSON). No proc-macro deps; testable standalone.
  Grammar surface: types, relations, `[user, group#member]` direct types,
  `or`/`and`/`but not`, `X from Y` (tuple-to-userset), conditions as CEL
  passthrough (embedded verbatim in JSON, no typed API in v1). ~600–900
  lines. **This is the main risk — derisk first.** Validation: vendor the
  openfga/language DSL↔JSON test corpus and snapshot round-trips (there is no
  official Rust parser — ours is a differentiator).
- New `r2e-openfga-macros`: `r2e_openfga::model!(pub mod authz = "fga/model.fga")`
  generates a typed module from the checked-in model file (emit an
  `include_str!` in the output for rebuild tracking):
  - `authz::MODEL` — the serialized JSON model, for boot-time apply/verify.
  - Per type: `authz::document::id(x) -> FgaObject<Ty>` (formats `type:id`,
    rejects `:` — same injection guard as today's resolver).
  - Per relation: `authz::document::viewer: FgaRel<Ty, Viewer>` — lowercase
    consts + `allow(non_upper_case_globals)`, same convention as `path::doc_id`.
  - `directly_related_user_types` encoded as `impl DirectlyAssignable<user::Ty>
    for Viewer` — typed writes check the subject type at compile time.
- Guard API: `FgaCheck::has(authz::document::viewer)` — one argument carries
  both relation and object type; a nonexistent relation is a compile error
  with a real span. The stringly `FgaCheck::relation("x").on("y")` form stays
  as the documented-unchecked escape hatch (dynamic models).
- Trybuild pass+fail fixtures in `r2e-compile-tests` (typo'd relation,
  relation on wrong type, disallowed subject type on grant).

### Phase 2 — Typed client + write-through invalidation — SHIPPED 2026-07-20

Landed as specified. Notes for later phases:
- Shape adjustment vs the sketch: `FgaClient` wraps **`OpenFgaRegistry`
  alone** (not `GrpcBackend` + registry). The tuple writes were added to the
  `OpenFgaBackend` trait itself (`write_tuple`/`delete_tuple`, **default
  impls returning `OpenFgaError::Unsupported`** so check-only custom
  backends keep compiling); `GrpcBackend` and `MockBackend` implement both,
  and the registry exposes its backend `pub(crate)`. Consequence:
  `FgaClient` is fully testable offline against `MockBackend` (now `Clone`,
  shared tuple set).
- `grant`/`revoke` bound `R: DirectlyAssignable<S::Marker>`; `check`
  deliberately does NOT (checks target computed relations).
- **`list_objects` DROPPED from the surface (user decision 2026-07-20).**
  It was implemented, reviewed and green, then removed: OpenFGA's
  `ListObjectsResponse` is a bare `repeated string objects` — the
  server-side bounds (`OPENFGA_LIST_OBJECTS_MAX_RESULTS`, deadline)
  silently return a *partial* list with no truncation flag or cursor
  (unlike SpiceDB's cursored `LookupResources`), so a typed
  `Vec<FgaObject<T>>` would read as exhaustive without being it. Revisit
  only on a real need, in this order: paginate-app-objects + `BatchCheck`
  helper (best candidate), `StreamedListObjects` (escapes the max-results
  cap, not the deadline), `Read`-paginated helper (direct tuples only).
  The dropped implementation (incl. `FgaObject::from_wire`, the
  `MalformedObject` error variant, tests and the example `list` endpoint)
  is recoverable from this session's diff history if upstream ever adds a
  truncation signal.
- Write-through invalidation is exact-object only (`invalidate_object`);
  transitive fan-out (userset grants) still needs `clear_cache()`/TTL —
  documented, unchanged from the registry's cache contract. The
  invalidate-after-write TOCTOU (a racing check can re-cache a stale
  decision until TTL) is documented on the registry, not fixed with cache
  versioning.
- OpenFGA `Write` semantics kept verbatim: duplicate grant / missing revoke
  = server error, not a no-op (no error-message parsing for idempotency).
- example-openfga exercises the client end-to-end (share/unshare endpoints,
  editor-gated; integration tests cover cached-deny → grant → allow).
  Fixtures: `compile-pass/fga_client_typed.rs`,
  `compile-fail/fga_client_grant_wrong_subject.rs`.
- Phase 3 can hand `FgaClient`/registry construction to the plugin — the
  three producers in example-openfga are what `.with(OpenFga::model(...))`
  should collapse.

### Phase 3 — `OpenFga` plugin + store lifecycle — SHIPPED 2026-07-20

Landed as specified (spelling: `.plugin(OpenFga::model(authz::MODEL))` — it
provides beans, so it is a `PreStatePlugin`, not a post-state `.with()`).
Implementation notes:

- **Shape:** `Provided = (OpenFgaRegistry, FgaClient, OpenFgaHandle)`. The
  registry is built at install over an internal `LazyBackend`
  (`Arc<OnceLock<GrpcBackend>>`); the async boot runs as `OpenFgaHandle`'s
  `PostConstruct` (via `ctx.run_post_construct`) inside `build_state()` —
  connect → resolve store → apply/verify model → pin `model_id` → fill the
  slot. Failure aborts startup; pre-boot backend use returns the new
  `OpenFgaError::NotReady` (unreachable in a normal boot).
- **Config** (`OpenFgaPluginConfig`, prefix `openfga`, all fields `Option` +
  install-time validation): `endpoint` (required), `store` (name) or
  `store_id`, `apply_model` (default true), `model_id` (verify-mode pin only
  — rejected in apply mode), `api_token`, timeouts, cache knobs. Read at
  **install** (typed `Config` at `configure` is too late — it runs after
  post-construct), so `.plugin()` must follow `load_config()` (panics with
  guidance otherwise). `openfga.enabled: false` skips the boot (fail-closed
  checks).
- **Store-name duplicates** (FGA names are not unique): apply mode picks the
  oldest (min id = oldest ULID) with a warning; verify mode errors → set
  `store_id`.
- **Model compare** lives in `r2e_openfga::model_convert`: `compile_model`
  (schema-1.1 JSON → prost via the `r2e-openfga-model` AST — the prost serde
  derives can't parse the official JSON, oneof tagging differs),
  `models_equal`/`diff_summary` on canonicalized forms (strip model id,
  `module`/`source_info`, empty-vs-absent metadata; sort type defs).
  Identical latest model → reuse (no append); different → write new version.
- Boot-time store/model RPCs go through the raw client with
  `request_with_token` (Bearer metadata is NOT auto-injected on raw calls);
  `GrpcBackend::from_parts` assembles the final backend from the connected
  channel + resolved ids.
- example-openfga migrated: producers + `document_model()` deleted; yaml is
  `endpoint` + `store: "documents"`. Tests: plugin does the bootstrap —
  unique store name per test (session-shared `DevOpenFga` container), seed
  tuples via the typed `FgaClient` bean. `DevOpenFga`'s HTTP bootstrap
  helpers remain for non-plugin flows. Lifecycle tests:
  `r2e-openfga/tests/plugin.rs` (apply/reuse/append/verify-ok/mismatch/
  missing-store + typed grant/check through the plugin beans).

### Phase 4 — CLI (later, lowest priority)

`r2e fga diff` / `push` / `pull` (diff local model vs store, pull an existing
store's model into a local `.fga`), tuple seed fixtures for dev/tests.

**Decisions taken with the proposal (2026-07-19):** DSL (`.fga`) is the
compile input, not JSON — requiring a `fga model transform` pre-step breaks
the promise (JSON may be accepted *additionally*). Conditions (schema 1.2):
parser-tolerant passthrough only in v1. Phase order: 1 first — it carries
both DX axes (compile-time + IDE completion on `authz::…`) and fronts the
only real risk (the parser); 2–3 consume the same generated markers.

## Tech debt (deferred, low priority)

- **Event bus perf** (2026-03 audit): superseded by W8 — the two remaining
  micro-opts (`Arc<EventMetadata>` fan-out sharing + lazy `EventMetadata::new()`
  on zero-subscriber emit) SHIPPED 2026-07-20 (breaking: `EventEnvelope.metadata`
  is now `Arc<EventMetadata>`). See `eventbus-perf.md` § Explicitly deferred.
- **gRPC trybuild fixture** — RESOLVED 2026-07-18 with the gRPC/proto
  automagic work (W11): fixtures now use real generated code from
  `r2e-compile-tests/proto/ping.proto`.

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
