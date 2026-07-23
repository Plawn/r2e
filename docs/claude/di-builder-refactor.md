# DI & Builder — Reference

The DI subsystem and `AppBuilder` as they exist today: unified registration,
HList state, compile-checked dependencies, and feature modules. (This file was
originally a refactor roadmap; the phased plan is complete and has been pruned —
see git history for the phase logs. Open work is listed at the bottom.)

## Registration API

- **`.register::<T>()`** — the single registration entry point. Backed by the
  `Registrable` trait (`r2e-core/src/beans.rs`): `type Provided; type Deps;
  fn register_into(&mut BeanRegistry)`. `#[bean]`/`#[derive(Bean)]`/async beans
  give `Provided = Self`; `#[producer]` gives `Provided = Output`. There is no
  `with_bean` / `with_async_bean` / `with_producer` — the user never has to know
  which trait the macro generated.
  Signature: `.register::<T>() -> AppBuilder<NoState, TCons<T::Provided, P>,
  <R as TAppend<T::Deps>>::Output>`.
- **`.provide(value)`** — register an already-built value.
- **`.register_override::<T>()`** — overrides a default registration without
  adding a duplicate `P` slot (`builder/nostate.rs`).
- **`.register_controllers::<(A, B, ...)>()`** — tuple fan-out (arity 1..=16)
  over `register_meta` / `from_context` / `routes` / `scheduled_tasks_boxed` /
  `register_consumers`. Registration lives on extension traits
  `RegisterController` / `RegisterControllers` (`builder/registration.rs`), so
  the index witnesses sit on the trait and are inferred at call sites.
- **Conditional registration**: `.when(cond, |b| ...)` plus the predicates
  `config_flag(&self, key) -> bool` and `profile_is(&self, profile) -> bool`.
  There is no `with_*_when` / `_on_config` / `_for_profile` / `with_alternative_*`
  zoo. Note that a runtime-conditional registration cannot be tracked in `P`;
  the compile-time-safe way to express "maybe present" is
  `#[producer] -> Option<T>` (the slot is always in `P`).
- `RegMeta` (`beans.rs`) unifies eager (`BeanRegistration`), lazy
  (`LazyBeanRegistration`), and fingerprint (`FingerprintReg`) registrations so
  dedup / alternative resolution / topological sort are written once.

## State: the HList model

There is no hand-written state struct and no typed-state path — the HList *is*
the state model.

- **`type_list.rs`** — value-level `HNil` / `HCons`; `HasBean<T, Idx>`
  (fixed-offset monomorphized access, friendly `on_unimplemented`); witness-free
  `BeanAccess::get` (`state.get::<T>()`; deliberately **not** in the prelude —
  its blanket `get` would shadow `Deref`-reached inherent `get`s); `BuildHList`
  (materializes `P` from the resolved `BeanContext`, one `ctx.get` per slot at
  startup); `BeanLookup` (witness-free dynamic access via a monomorphized
  TypeId-compare chain — the vocabulary for guards, interceptors, and
  `ManagedResource`).
- **`build_state()`** takes no type arguments: it materializes `P` into the HList
  state and retains the graph as `Arc<BeanContext>` through the typed phase
  (`bean_context()` / `state()` accessors). Dev-reload caches `(state, ctx)`.
  The typed phase holds `state: T` (not `Option<T>`), so `register_controller`
  has no `.expect("state must be set")`.
- Apps with more than ~127 registrations need `#![recursion_limit = "512"]` at
  the crate root; `r2e doctor` warns as the bean count approaches it.

## Controller / endpoint wiring

- **`ContextConstruct`** — `from_context(ctx)` pulls each `#[inject]` field by
  type (`ctx.get::<Ty>()`) and declares `type Deps`.
- The generated `Controller<S, W>` impl is generic over the state
  (`S: Clone + Send + Sync + 'static + BeanLookup`), with `W` carrying inferred
  extraction markers.
- **`EndpointDeps`** (`r2e-core/src/controller.rs`) is the transport-neutral,
  state-independent carrier of a controller's *full* dependency fold — core
  `#[inject]` types + `R2eConfig` + decorator (guard/interceptor) deps.
  `#[routes]` and `#[grpc_routes]` both emit it; `register_controller`,
  `register_grpc_service`, and the module-scope check all bind it via
  `AllSatisfied`, so every registration scope (HTTP, module, scheduled, gRPC) is
  compile-checked — a missing bean is a compile error naming the type. Recipe
  for new wire adapters: `transport-adapters.md`.
- Guards/interceptors do not read the state; they are built once at registration
  from `DecoratorSpec`, deps folded into `EndpointDeps` — see
  `guards-interceptors.md`.

## Extraction (`r2e-core/src/extract.rs`)

`FromRequestPartsVia<S, M>` / `OptionalFromRequestPartsVia<S, M>` — R2E-owned
extraction traits with a marker slot `M` where bean-backed extractors park their
`HasBean` index witnesses (works around E0207). A blanket `ViaAxum` bridge
covers plain axum extractors; `Via<T, M>` adapts inside generated closures;
`BeanExtract<T, I>` serves hand-written handlers. `r2e-security` extracts via
`HasBean<Arc<JwtClaimsValidator>, I>` parked in `ViaBean<I>`.

**Overlap invariant (actively checked, not structural).** A type must NOT
implement both axum's `FromRequestParts`/`OptionalFromRequestParts` generically
and R2E's `FromRequestPartsVia`/`OptionalFromRequestPartsVia`. This cannot be
excluded by construction: both "sealed marker discipline" and "deterministic
marker selection" bottom out in a negative trait bound, which stable Rust cannot
express, and any blanket re-bridge just moves the overlap one level down.
Instead, `assert_unambiguous_extractor::<S, T, M>()` is an inference probe that
compiles iff `T` has exactly one extraction route against `S`. All first-party
bean-backed extractors are pinned with it (`r2e-core/tests/`,
`r2e-security/tests/extractor.rs`, `claims_identity_macro.rs`), it is documented
as the authoring tool for third-party extractors (module docs in `extract.rs` +
the book's `advanced/macro-debugging.md` troubleshooting table), and the failure
modes are pinned by trybuild (`extractor_dual_route_probe.rs`,
`extractor_dual_route_ambiguous.rs`).
**Do NOT re-add a blanket `OptionalFromRequestPartsVia<_, ViaAxum>` bridge.**

## Feature modules (`r2e-core/src/module.rs`)

Spring/NestJS-style module bundles with **compile-time encapsulation** those
frameworks cannot offer.

- `FeatureModule` is declarative — `Providers` / `Controllers` / `Exports` /
  `Imports`, no register body. `BeanList` folds provided types + aggregate deps +
  registration from `Registrable`; `ControllerDepsList` folds each controller's
  `EndpointDeps::Deps`; `ModuleControllers` / `ModuleList` carry the deferred
  controller folds.
- `AppBuilder` carries a 4th phantom param `Mods` (default `TNil`).
  `register_module` (extension trait `RegisterModule`, witnesses inferred)
  registers providers into the global graph, grows `P` by `Exports` **only** and
  `R` by `Imports` **only**, and queues the module. `build_state()` folds `Mods`
  after materializing the state through an *unchecked* registration backend —
  module controllers may inject private beans, since cores construct from the
  retained `BeanContext`. `with_state` is restricted to `Mods = TNil`;
  `RawPreStatePlugin::install` carries the `Mods` type param.
- Encapsulation is enforced by dedicated check traits with module-targeted
  diagnostics: `InModuleScope` / `ModuleDepsSatisfied` (deps ⊆ Provides ∪
  Imports) and `ProvidedByModule` / `ExportsProvided` (exports must be
  provided). Trybuild covers: provider dep out of scope, export not provided,
  controller dep out of scope, private bean invisible to app controllers.
- `#[module(providers(...), controllers(...), exports(...), imports(...),
  requires_plugins(...))]` generates the `FeatureModule` impl; all keys optional.
- **Module-imports-module composition.** An `imports(...)` entry is either a bean
  type or `module(OtherModule)`, mixed freely (`imports(DbPool, module(Billing))`;
  `module(A, B)` and repeated `module(A), module(B)` are equivalent). The macro
  appends each imported module's `Exports` to `Imports` via `TAppend`, so the
  generated `type Imports` is e.g.
  `<TCons<DbPool, TNil> as TAppend<<Billing as FeatureModule>::Exports>>::Output`
  (multiple modules chain the appends). This is macro-only — `module.rs` is
  untouched. Importing a module **only requires its exports**; it does NOT
  register the module — the app must still `.register_module::<Billing>()`
  (deliberate: two modules importing the same one don't double-register →
  `DuplicateBean`). `module(...)` in any other key is a targeted macro error.
- Same-typed **private** beans in different modules collide at runtime
  (`DuplicateBean` at startup, by design — the graph is `TypeId`-keyed). Use
  newtypes.

## Design decisions worth not relitigating

- **Duplicate bean detection stays runtime.** A spike proved compile-time
  detection is feasible on stable Rust via an inference-ambiguity trick, but it
  re-introduces a `_` witness on `.register::<T, _>()`, produces a cryptic E0283
  that `#[diagnostic::on_unimplemented]` cannot improve (it's an ambiguity, not
  an unsatisfied bound), rejects the intentional default/override pattern, taxes
  every generic wrapper over `P`, and is incompatible with
  `allow_bean_override()`. The runtime `DuplicateBean` check (the first thing
  `build_state` does, `beans.rs`) has a `Display` message pointing at the fix.
- **Inherently runtime, cannot be moved to compile time:** dependency **cycles**
  (would need the whole graph at the type level — softened instead:
  `topological_sort` runs a three-color DFS `find_cycle` over the stuck
  `in_degree > 0` subgraph and reports one concrete `A -> B -> A` path) and
  **missing config keys** (YAML is read at startup — softened to a `Result`:
  `try_register_controller() -> Result<Self, ConfigValidationError>`).
- **Qualifiers rejected** — newtypes by design (see `roadmap.md`).

## Remaining work

The DI/builder backlog has landed (including first-class module-imports-module
composition via `imports(module(...))`, above); the live backlog is
`docs/claude/roadmap.md`.
