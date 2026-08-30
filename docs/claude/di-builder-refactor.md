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
- **`BeanState<L>`** — the materialized list `L` behind ONE `Arc`, and what is
  actually installed as the router state (`build_state()` returns
  `AppBuilder<BeanState<<P as BuildHList>::Output>>`). The backend clones the
  router state on every request whether or not the handler declares `State<S>`,
  so installing the bare `HCons` chain cost one bean `Clone` per bean per
  request — O(N) in the width of the graph. The wrapper makes it one refcount
  bump (task #992, `docs/claude/hot-path-clone-audit.md`). It forwards
  `HasBean<T, Idx>` (index witness intact — so `state.get::<T>()` is still one
  pointer deref plus a constant field offset, no `TypeId` lookup), `Contains`
  (so every `AllSatisfied<StateType, _>` bound sees through it), `BeanLookup`,
  and `Deref<Target = L>`. `BeanAccess::get` comes free from its blanket impl.
  Nothing in `r2e-macros` changed: the generated extractor and the
  `Controller<S, W>` impl were already state-generic.
- **`build_state()`** takes no type arguments: it materializes `P` into the HList
  state, wraps it in `BeanState`, and retains the graph as `Arc<BeanContext>`
  through the typed phase (`bean_context()` / `state()` accessors).
  `BuildHList::build_bean_state` is the wrapping form of `build_hlist`.
  Dev-reload caches `(state, ctx)`.
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

## Extraction (`r2e-core/src/web/extract.rs`)

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

## Feature modules (`r2e-core/src/di/module.rs`)

Spring/NestJS-style module bundles with **compile-time encapsulation** those
frameworks cannot offer.

- `FeatureModule` is declarative — `Providers` / `Controllers` / `Exports` /
  `Imports` / `RequiredPlugins` / `Plugins`, no register body. `BeanList` folds provided types + aggregate deps +
  registration from `Registrable`; `ControllerDepsList` folds each controller's
  `EndpointDeps::Deps`; `ModuleControllers` / `ModuleList` carry the deferred
  controller folds.
- `AppBuilder` carries a 4th phantom param `Mods` (default `TNil`).
  `register_module` (extension trait `RegisterModule`, witnesses inferred)
  installs the plugins the module **brings** (`Plugins`, below), registers
  providers into the global graph, grows `P` by `Exports` (plus the brought
  plugins' provisions) and `R` by `Imports` (plus their `Deps`), and queues the
  module. `build_state()` folds `Mods`
  after materializing the state through an *unchecked* registration backend —
  module controllers may inject private beans, since cores construct from the
  retained `BeanContext`. `with_state` is restricted to `Mods = TNil`;
  `PluginInstall::install` carries the `Mods` type param.
- Encapsulation is enforced by dedicated check traits with module-targeted
  diagnostics: `InModuleScope` / `ModuleDepsSatisfied` (deps ⊆ Provides ∪
  Imports) and `ProvidedByModule` / `ExportsProvided` (exports must be
  provided). Trybuild covers: provider dep out of scope, export not provided,
  controller dep out of scope, private bean invisible to app controllers.
- `#[module(providers(...), controllers(...), grpc_services(...), exports(...),
  imports(...), plugins(...), requires_plugins(...))]` generates the
  `FeatureModule` impl; all keys optional.
- **Modules bring their plugins.** `plugins(Scheduler = Scheduler)` (macro) /
  `type Plugins = (Scheduler,)` + `fn plugins()` installs the plugin at the
  `register_module` call site; `requires_plugins(Scheduler)` only *needs* it
  installed elsewhere. The `Type = expr` form is required (bare type / missing
  `=` are targeted macro errors): the type grows the provision list at compile
  time, the expression is the instance. `ModulePluginList` (P/R-independent,
  feeds `ModuleScope`) + `ModulePlugins<P, R, Mods>` (the value fold whose
  `OutP`/`OutR`/`OutMods` are exactly what a sequential `.plugin(a).plugin(b)`
  chain yields) do the work, so brought provisions are app-global, brought
  controllers are queued, and brought effects apply at that call's position in
  install order. A brought plugin's bean must **not** be listed in `Exports`
  (it is already in `P`; two slots for one type breaks `HasBean` inference), and
  `requires_plugins` is checked against the **post-fold** `P` — so a module can
  both bring and require, and a later module can be satisfied by an earlier
  one's plugin. One owner per plugin: a double install (app + module, or two
  modules) is `BeanError::DuplicatePlugin`, naming both owners and pointing at
  `requires_plugins`. Stable Rust has no associated-type defaults, so
  hand-written impls must write `type Plugins = (); fn plugins() {}`.
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
- **Modules own non-HTTP endpoints too** (ticket #989). `FeatureModule` has a
  transport-neutral `type Endpoints: ModuleEndpointSet` — the peer of
  `Controllers`. r2e-core stays transport-agnostic by splitting the hook in two:
  `ModuleEndpointSet` exposes only the aggregated `type Deps` (checked against
  `ModuleScope<M>` at `register_module`, exactly like `M::Controllers`), and
  `ModuleEndpoints<T>::register_all` does the value-level registration inside the
  `ModuleList` fold that `build_state()` applies — after the module's
  controllers, from the same retained `BeanContext`, through an **unchecked**
  backend (a module endpoint may inject private beans, so the app-state
  `AllSatisfied` check would wrongly reject it). Both traits are implemented in
  the transport crate: `r2e_grpc::ModuleGrpcServices<(A, B)>` (a local wrapper —
  orphan rules forbid implementing a foreign trait for a bare tuple of type
  params) folds each service's `EndpointDeps::Deps` with `TAppend` and registers
  them in declaration order. `#[module(grpc_services(A, B))]` generates exactly
  that, and appends `GrpcServer` to `RequiredPlugins`, so a missing plugin is the
  standard module diagnostic naming `GrpcServer` instead of a boot-time failure.
  `RequiredPluginInstalled` checks the plugin's *provisions*, not its identity,
  so that claim only holds because `GrpcMarker` is unconstructible outside
  r2e-grpc (deliberate: a hand-`.provide(GrpcMarker)` would otherwise compile a
  module with no registry to register into). With no `grpc_services` key the
  macro emits `type Endpoints = ();` and no r2e-grpc path, so modules compile in
  apps without the gRPC feature. Failures go to the boot error channel
  (`BeanError::EndpointConfig` / `MissingTransportPlugin` / `DuplicateEndpoint`,
  the last two naming the declaring module) so `try_build_state()` stays
  non-panicking. Endpoint names are unique across the whole app: the registry
  refuses a name it already holds, so two modules claiming one service, or a
  module plus an app-level `.register_grpc_service::<S>()`, fails at boot instead
  of double-registering (a repeat inside one `grpc_services(..)` is a macro
  error). Stable
  Rust has no associated-type defaults, so hand-written impls must write
  `type Endpoints = ();`. Trybuild covers the missing plugin, an endpoint dep out
  of module scope, and the happy path; `examples/example-grpc/tests/grpc_module.rs`
  serves a module-owned service over the wire.
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
