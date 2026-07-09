# DI & Builder Refactor — DX + Compile-Time Roadmap

Status: **Phases 1–5 complete** (landed on `refactor/di-builder-dx-ct`).
Tracks a multi-phase refactor of the DI subsystem and `AppBuilder`. Each
phase ends with a quality-review gate before the next starts.

| Phase | Item | Status |
|---|---|---|
| 0 | RegMeta foundation | ✅ done |
| 1a | Unified `.register::<T>()` | ✅ done |
| 1b | Fused witnesses + `build_state!` | ✅ done |
| 1c | Guaranteed `state: T` | ✅ done |
| 1d | `.when()` + collapsed zoo | ✅ done |
| 1e | Duplicate detection | ✅ decided: runtime |
| 1f | Controller tuples | ✅ done |
| 2a | `BuiltApp<T>` struct | ✅ done |
| 2b | Split `builder.rs` | ✅ done |
| 3 | Correctness & cleanup | ✅ done |
| 4 | Controllers as graph-resolved beans | ✅ done — A3 landed; HList state is the single state model; review gate passed (no blocking findings; ambiguity + guard-panic fixes applied), see `plan-controllers-as-beans.md` |
| 5 | Feature modules (closed subgraphs) | ✅ done — `FeatureModule` + `register_module` + `#[module]`; compile-time encapsulation (deps ⊆ Provides ∪ Imports, exports-only leakage to `P`); see `plan-feature-modules.md` |
| 6 | Guards & interceptors as graph-resolved decorators | ✅ done — `Guard<I>`/`PreAuthGuard`/`Interceptor<R>` lost the state param; `DecoratorSpec` + `SelfBuilt`; sites built once at wiring time, deps folded into `Controller::Deps` (compile-checked); `cache_backend()` global deleted; see `plan-guards-as-beans.md` |

**Next steps:** the prioritized post-Phase-6 backlog is in `di-next-steps.md`
(bridge-overlap invariant, spec DX, scheduled/gRPC ctx; qualifiers rejected —
newtypes by design). Item 1 (module decorator-deps carrier) landed 2026-07-08:
`#[routes]` emits a state-independent carrier of the full dep fold, consumed
by both `Controller::Deps` and the module-scope check. Item 10 (2026-07-09)
promoted that carrier to the transport-neutral **`EndpointDeps`** (renamed
from `ControllerDeps`): `#[grpc_routes]` emits it too and
`register_grpc_service` gained the `AllSatisfied` bound — every registration
scope (HTTP, module, scheduled, gRPC) is now compile-checked. Recipe for
future wire adapters: `transport-adapters.md`.

Phase 1 shipped a clean quality-review gate (no correctness bugs found) and a
46-file docs alignment pass.

## Motivation

The DI audit surfaced **partial compile-time safety with runtime escape
hatches**, plus a noisy builder API:

- Three parallel registration methods (`with_bean` / `with_async_bean` /
  `with_producer`) — the user must know which trait a macro generated and match
  the right method.
- ~18 conditional methods (`with_*_when` / `_on_config` / `_for_profile` /
  `with_alternative_*`) — huge, heavily-documented surface. Critically,
  `with_bean_when` returns `Self`, so the type **drops out of the provision list
  `P`** — this breaks compile-time tracking and forces a runtime
  `MissingDependency`.
- `build_state::<S, _, _>()` — two opaque witness parameters leak into every
  call site.
- `register_controller` called once per controller (vertical noise) plus a
  `.expect("state must be set")` runtime check even though the typed builder
  phase should guarantee it.
- `build_inner` returns a **10-element tuple** (clippy `type_complexity`); and
  `builder.rs` is a **1981-LOC** monolith.

Goals: (1) clean up builder DX, (2) move as many runtime errors as possible to
compile time. Breaking changes are allowed (R2E is pre-production); no
deprecation shims. Work proceeds phase by phase with a review gate between.

### What is genuinely movable runtime → compile-time

| Runtime error today | Movable? | How |
|---|---|---|
| `MissingDependency` via `with_bean_when` | ✅ the big one | `with_bean_when` returns `Self`, dropping the type from `P`. Removing the conditional zoo (1d) and steering to `#[producer] -> Option<T>` (slot always in `P`) turns "runtime missing dep" into "compile-time present". |
| `.expect("state must be set")` in `register_controller` | ✅ | Make the typed phase hold `state: T` (not `Option<T>`); the runtime check vanishes structurally. |
| `DuplicateBean` (runtime) | ⚠️ spike | Type-level uniqueness check on `P`. Hard on stable Rust (no negative bounds); spike with runtime fallback. |
| `_,_` in `build_state::<S,_,_>()` | ✅ ergonomics | Fuse `BuildableFrom` + `AllSatisfied`; fold `BeanState` field requirements into `R`. |
| Wrong `with_bean` vs `with_async_bean` | ✅ DX | Unified `.register::<T>()`. |

Inherently runtime (cannot move): dependency **cycles** (would need the whole
graph at the type level), and **missing config keys** (YAML read at startup) —
but both can be softened (better cycle message; panic → `Result`).

The real compile-time lever is **1d** (close the `with_bean_when` hole) + **1c**
(guaranteed state), not 1e.

## Foundation (done)

`RegMeta` trait in `r2e-core/src/beans.rs` unifies eager
(`BeanRegistration`), lazy (`LazyBeanRegistration`), and fingerprint
(`FingerprintReg`) registrations so dedup / alternative resolution / topological
sort are written once. Committed. `Registrable` and `describe_graph` build on it.

## Phase 1 — Builder DX + compile-time (priority) ✅ COMPLETE

### 1a — Unified `.register::<T>()`
- New `Registrable` trait in `beans.rs`: `type Provided; type Deps;
  fn register_into(&mut BeanRegistry)`. Bean/AsyncBean → `Provided = Self`;
  Producer → `Provided = Output`.
- Macros emit `impl Registrable for T` (inherent per-type impl, no blanket
  overlap): `bean_attr.rs`, `bean_derive.rs`, `producer_attr.rs`.
- Builder: `.register::<T>() -> AppBuilder<NoState, TCons<T::Provided, P>,
  <R as TAppend<T::Deps>>::Output>`.
- **Breaking**: remove `with_bean` / `with_async_bean` / `with_producer`.

### 1b — Remove `_,_` from `build_state`
- Fuse `BuildableFrom` (`type_list.rs:170`) and `AllSatisfied`
  (`type_list.rs:211`) into a single mechanism (both already rest on
  `Contains`, `type_list.rs:126`).
- `#[derive(BeanState)]` (`bean_state_derive.rs`) stops emitting `BuildableFrom`
  and instead exposes field types as a `TCons` requirements list (via
  `build_tcons_type`, `type_list_gen.rs:8`) folded into `R`. Reconcile witness
  encoding: `BuildableFrom` uses a flat tuple, `AllSatisfied` a cons-nested
  pair — target the cons-nested form.
- `build_state<S, W>` → one witness; add a `build_state!(app, S)` macro façade
  for zero underscores.
- **Ripple**: hand-written `BuildableFrom` impls in `integration.rs:307/553/576`
  and `r2e-prometheus/tests/plugin.rs:21`.

### 1c — Guarantee `state: T` in the typed phase
- Typed `AppBuilder<T>` currently stores `state: Option<T>`;
  `register_controller` `.expect`s it (builder.rs:1134).
- Make the typed phase hold `state: T`; the NoState→T transition
  (`build_state` / `with_state`) produces a concrete state. Removes the
  `.expect`. Ripple: `from_pre` (dev-reload) and typed-phase constructors.

### 1d — Collapse the conditional zoo (the compile-time lever)
- Remove the ~18 conditional methods (builder.rs:266-457).
- Add `.when(cond: bool, f: impl FnOnce(Self) -> Self) -> Self` + helpers
  `config_flag(&self, key) -> bool`, `profile_is(&self, p) -> bool` (reuse
  `is_config_enabled`, `active_profile`).
- Runtime-flag conditional presence cannot be compile-time tracked; the only
  compile-time-safe path is `#[producer] -> Option<T>` (slot always in `P`).
  Keep the default/alternative pattern (distinct override semantics).

### 1e — Duplicate detection: RUNTIME (decided)
- A spike proved compile-time detection IS feasible on stable Rust via an
  inference-ambiguity trick, but the costs outweigh the benefit:
  - it adds a `_` witness to `.register::<T, _>()` (undoes 1b's zero-underscore
    goal);
  - the error is a cryptic E0283 "type annotations needed" that
    `#[diagnostic::on_unimplemented]` cannot improve (it's an ambiguity, not an
    unsatisfied bound);
  - it rejects the intentional default/override pattern (kept in 1d) and would
    require a dual `.override_bean::<T>()`;
  - it taxes every generic wrapper over `P` and is incompatible with the runtime
    `allow_bean_override()` mode.
- The only error it catches — the same type registered twice — is already caught
  loudly at startup by the runtime `DuplicateBean` check (the first thing
  `build_state` does), with a clear `Display` message.
- **Decision: keep the runtime check.** Its message was improved to point at the
  fix (remove the duplicate, or use `.with_default_bean()` / `.allow_bean_override()`
  for intentional overrides). No type-level change.

### 1f — Controller tuples
- `register_controllers::<(A, B, ...)>()` via a trait implemented for tuples of
  arity 1..=16, mirroring `impl_plugin_deps!` (`type_list.rs:276-319`). Fans out
  `register_meta` / `from_state` / `routes` / `scheduled_tasks_boxed` /
  `register_consumers` per element (builder.rs:1116-1165).

## Phase 2 — Builder structure

### 2a — `BuiltApp<T>` instead of the 10-tuple ✅
Replaced `build_inner`'s 10-element tuple return with a private `BuiltApp<T>`
struct; `build()` takes `.router`, `prepare()` destructures it into `PreparedApp`.

### 2b — Split `builder.rs` (1981 LOC) ✅
Pure moves, no API change: `builder/nostate.rs` (NoState phase),
`builder/typed.rs` (typed phase + `BuiltApp`), `builder/prepared.rs`
(`PreparedApp` + `ServeStrategy`), `builder/task_registry.rs`
(`TaskRegistryHandle`), `builder/mod.rs` (`AppBuilder`, `NoState`,
`BuilderConfig`, shared type aliases, `build_state!` macros). Children use
`use super::*`; `from_pre` and `PreparedApp` fields are `pub(super)`.

## Phase 3 — Correctness & cleanup ✅

All landed; `cargo clippy -p r2e-core` is at **0 warnings including
`--all-features`**. Review gate passed (no correctness findings).

- **Readable `build_state` panic**: `.unwrap_or_else(|e| panic!("...: {e}"))` —
  `Display` exists (beans.rs:525) but `.expect` used `Debug` and swallowed it.
- **Real cycle path**: `topological_sort` now calls `find_cycle` (three-color
  DFS over the stuck `in_degree>0` subgraph) reporting one concrete
  `A -> B -> A` path.
- **panic → Result**: `try_register_controller() -> Result<Self,
  ConfigValidationError>`; `register_controller` wraps it (ends the panic in
  `builder/typed.rs` `register_controller`).
- **`result_large_err`**: `Result<(), Box<Response>>` in `validation.rs:37/53`
  (+ `.map_err(Box::new)` at :44) and deref in the generated handler
  (`r2e-macros/src/codegen/handlers.rs:169`: `return *__validation_err;`).
- **Clippy**: `mem::take` (`builder/nostate.rs` `try_build_state` + dev-reload
  path), `question_mark` (secrets.rs:51), `manual_async_fn` (request_id.rs:55),
  feature-gated `map_flatten` (`builder/prepared.rs` `run_inner`, `quic`) and
  `while_let_loop` (multipart.rs:162, `multipart`). `BuiltApp` cleared one
  `type_complexity`; others get type aliases.

## Verification

- `cargo check --workspace` + `cargo check -p r2e-core --features dev-reload`.
- `cargo clippy -p r2e-core` (target: 0 core warnings).
- `cargo test --workspace` — watch `integration.rs` and
  `r2e-prometheus/tests/plugin.rs:21` (hand-written `BuildableFrom` impls to
  update after the 1b fusion).
- `cargo run -p example-app` — migrate its assembly (`.provide` ×6,
  `register_controller` ×12, `with_bean/producer`) to `.register()` /
  `register_controllers!` / `build_state!`; confirm it serves on `0.0.0.0:3000`.
- `r2e-compile-tests` (trybuild) — update expected error messages for removed
  methods and new compile errors.

## Phase 4 — Controllers as graph-resolved beans ✅ COMPLETE (A3)

Landed as designed in `plan-controllers-as-beans.md` (approach A3), with the
user decision applied: **the typed state path was removed entirely** — HList
state is the single state model.

- **4a — HList machinery** (`type_list.rs`): value-level `HNil`/`HCons`;
  `HasBean<T, Idx>` (fixed-offset monomorphized access, friendly
  `on_unimplemented`); witness-free `BeanAccess::get` (`state.get::<T>()`, NOT
  in the prelude — its blanket `get` would shadow `Deref`-reached inherent
  `get`s); `BuildHList` (materializes `P` from the resolved `BeanContext`, one
  `ctx.get` per slot at startup); `BeanLookup` (witness-free dynamic access —
  monomorphized TypeId-compare chain — the vocabulary for guards, interceptors,
  `ManagedResource`).
- **4b — Builder**: `build_state()` (no type args) materializes `P` into the
  HList state and retains the graph as `Arc<BeanContext>` through the typed
  phase (`bean_context()` / `state()` accessors); dev-reload caches
  `(state, ctx)`. `register_override::<T>()` overrides a default registration
  without adding a duplicate `P` slot.
- **4c — Codegen**: `ContextConstruct` (by-type `ctx.get`) replaces
  `StatefulConstruct` (removed); the generated `Controller<S, W>` impl is
  generic over the state (`S: Clone + Send + Sync + 'static + BeanLookup`),
  with `W` carrying inferred extraction markers. `Controller::Deps` (unique
  `#[inject]` types + `R2eConfig`) is checked via `AllSatisfied` at
  `register_controller` — a missing bean is a compile error naming the type.
  Registration moved to extension traits `RegisterController` /
  `RegisterControllers` (witnesses on the trait, inferred at call sites).
- **4d — Extraction** (`r2e-core/src/extract.rs`): `FromRequestPartsVia<S, M>`
  / `OptionalFromRequestPartsVia<S, M>` with a marker slot for `HasBean`
  witnesses (E0207); blanket `ViaAxum` bridge for plain axum extractors;
  `Via<T, M>` adapter in generated closures; `BeanExtract<T, I>` for
  hand-written handlers. `r2e-security` extracts via
  `HasBean<Arc<JwtClaimsValidator>, I>` parked in `ViaBean<I>`.
- **4e — Removal + sweep**: `BeanState` (trait + derive), `build_typed_state`,
  `build_state!`/`try_build_state!`, `StatefulConstruct`,
  `#[controller(state = ...)]`, `#[service(state = ...)]` all removed
  (rejected with migration-hint compile errors where applicable).
  `ServiceComponent`/`#[derive(BackgroundService)]`, `register_subscriber`,
  and gRPC services construct from the context by type. All examples, tests,
  CLI templates migrated; `r2e doctor` warns when the bean count approaches
  the `#![recursion_limit = "512"]` threshold (>~127 registrations).

Revisits **1b/1c** as predicted: `build_state!` and `BeanState::Requires` are
gone (the state type is inferred from `P`); the `P`/`AllSatisfied` presence
tracking and the guaranteed `state: T` phase remain.

### Phase 4 tech debt (deferred, recorded 2026-07-08)

1. **RESOLVED 2026-07-08** (di-next-steps item 2) — ~~Extraction-bridge
   overlap fragility.~~ The invariant (a type must NOT implement both axum's
   `FromRequestParts`/`OptionalFromRequestParts` generically and R2E's
   `FromRequestPartsVia`/`OptionalFromRequestPartsVia`) **cannot be excluded
   by construction**: both "sealed marker discipline" and "deterministic
   marker selection" bottom out in a negative trait bound ("`T` does NOT
   implement the axum trait"), which stable Rust cannot express, and any
   blanket re-bridge just moves the same overlap one level down. It is now an
   **actively checked** invariant instead of an implicit one:
   `r2e_core::extract::assert_unambiguous_extractor<S, T, M>()` is an
   inference probe that compiles iff `T` has exactly one extraction route
   against `S`; all first-party bean-backed extractors are pinned with it
   (`r2e-core/tests/extract.rs`, `r2e-security/tests/extractor.rs`,
   `claims_identity_macro.rs`), and it is documented as the authoring tool
   for third-party extractors (module docs in `extract.rs` + book
   troubleshooting table in `advanced/macro-debugging.md`). The failure mode
   is pinned by trybuild (`extractor_dual_route_probe.rs` — clean localized
   E0283 listing both impls; `extractor_dual_route_ambiguous.rs` — the
   real-world shape at `register_controller`, which also names both
   competing impls). Still true: do NOT re-add a blanket
   `OptionalFromRequestPartsVia<_, ViaAxum>` bridge.
2. **RESOLVED by Phase 6** (guards & interceptors as graph-resolved
   decorators, see `plan-guards-as-beans.md`) — original entry kept below
   for history. ~~Guard/interceptor bean deps are runtime-checked, not compile-checked.~~
   `RateLimitGuard`/`FgaGuard` (and bean-reading interceptors like a DB audit
   log) fetch their beans via `BeanLookup`; a missing bean is a per-request
   500 (clean, but runtime). The blocker is NOT E0207 (solvable by a marker
   slot on `Guard`, like `FromRequestPartsVia`): it is that `#[guard(expr)]`
   takes an arbitrary **expression whose type the macro cannot name**, and
   every witness-threading bound needs the type name (identity params work
   because the parameter has a declared type in the signature). Moving this to
   compile time requires an API redesign — candidates: (a) type-naming syntax
   `#[guard(RateLimitGuard = RateLimit::per_user(5, 60))]`; (b) guards as
   beans/factories resolved from the graph (deps become `Deps`, checked by
   `AllSatisfied`); (c) marker slot + typed syntax. Cheap intermediate
   improvement available without redesign: evaluate guard expressions once at
   router-build time and add a `Guard::startup_check(&state)` hook so
   misconfiguration fails at **boot**, not on first request.
   → **Design proposed** in `plan-guards-as-beans.md` (Phase 6): guards and
   interceptors as graph-resolved decorators, compile-checked deps,
   once-at-registration construction; supersedes the `startup_check` idea.

## Phase 5 — Feature modules ✅ COMPLETE

Landed as designed in `plan-feature-modules.md` (spike decisions recorded
there). Spring/NestJS-style module bundles with **compile-time encapsulation**
Spring/NestJS cannot offer:

- **5a** — `r2e-core/src/module.rs`: declarative `FeatureModule`
  (`Providers`/`Controllers`/`Exports`/`Imports`; no register body),
  `BeanList` fold (derives provided types + aggregate deps + registration
  from `Registrable`), `ControllerDepsList` (state-independent controller
  deps — originally via `ContextConstruct::Deps`; since the post-Phase-6
  carrier — now `EndpointDeps` — it folds the full list incl. decorator deps),
  `ModuleControllers`/`ModuleList`
  (deferred controller folds). `AppBuilder` gained a 4th phantom param
  `Mods` (default `TNil`); `register_module` (extension trait
  `RegisterModule`, witnesses inferred) registers providers into the global
  graph, grows `P` by `Exports` only / `R` by `Imports` only, and queues the
  module; `build_state()` folds `Mods` after materializing the state through
  an **unchecked** registration backend (module controllers may inject
  private beans — cores construct from the retained `BeanContext`).
  Breaking: `RawPreStatePlugin::install` gained the `Mods` type param;
  `with_state` is restricted to `Mods = TNil`.
- **5b** — dedicated encapsulation-check traits with module-targeted
  diagnostics (`InModuleScope`/`ModuleDepsSatisfied`,
  `ProvidedByModule`/`ExportsProvided`); trybuild coverage for provider dep
  out of scope, export not provided, controller dep out of scope, private
  bean invisible to app controllers.
- **5c** — `#[module(providers(...), controllers(...), exports(...),
  imports(...))]` generates the `FeatureModule` impl (all keys optional).
- **5d** — example-app users slice migrated to `UserModule`; docs updated.

Same-typed **private** beans across modules collide at runtime
(`DuplicateBean` at startup, by design — the graph is `TypeId`-keyed): use
newtypes. Module-imports-module composition works via exported bean types; a
first-class "imports = [OtherModule]" form is future work.
