# DI — Next Improvement Axes (post-Phase 6)

Status: **BACKLOG** — recorded 2026-07-08, after Phase 6 (guards/interceptors
as graph-resolved decorators) landed. Hub: `di-builder-refactor.md` (phases
1–6 all complete). This file is the working list for the next DI session;
items are ordered by recommended priority.

## Recommended order

~~1~~ ~~3~~ ~~4~~ (done) → 2 → 5. Items 6/8/9 are opportunistic. Item 7 is
**rejected** (user decision — do not re-propose).

---

## 1. ~~Module controllers' decorator deps~~ — ✅ DONE (2026-07-08)

The last compile-check hole is closed. `#[routes]` emits a
state-independent carrier `impl r2e_core::ControllerDeps for <Name>
{ type Deps = <full fold> }` (trait in `r2e-core/src/controller.rs`,
`#[doc(hidden)]`; impl generated in `controller_impl.rs`) holding
`ContextConstruct::Deps ++ Σ DecoratorSpec::Deps`; the generated
`Controller<S, W>::Deps` points at it (single source of truth), and
`ControllerDepsList` (`r2e-core/src/module.rs`) bounds `ControllerDeps`
instead of `ContextConstruct`, so the module-scope check
(`Deps ⊆ Provides ∪ Imports`) now covers guards/interceptors. An
out-of-scope decorator dep is a compile error at `register_module` instead
of a `build_state()` panic.

Breaking: a hand-written controller placed in a `FeatureModule::Controllers`
tuple must implement `ControllerDeps` (i.e. use `#[routes]`). Note the check
is intentionally strict: example-app's `UserModule` had to declare
`RateLimitRegistry` + `Arc<dyn CacheStore>` in `imports` for
`UserController`'s decorators. Tests:
`module_controller_guard_dep_not_in_scope.rs` (compile-fail) /
`module_controller_guard_dep_in_scope.rs` (compile-pass).

## 2. Extraction-bridge overlap invariant (Phase 4 debt, item 1)

**Problem.** Since the `Option<T>` ambiguity fix removed the blanket
`OptionalFromRequestPartsVia<_, ViaAxum>` bridge, the invariant is
*implicit*: a type must NOT implement both axum's
`OptionalFromRequestParts` (generically) and R2E's
`OptionalFromRequestPartsVia`, or its `Option<T>` marker becomes ambiguous
(E0283) at `register_controller` — a cryptic error. Nothing enforces this;
the design relies on bean-backed extractors having no axum impls.

**Direction.** Either enforce it (sealed marker discipline) or make marker
selection deterministic. Do not silently re-add blanket bridges to
`r2e-core/src/extract.rs`.

**Origin:** `di-builder-refactor.md` § "Phase 4 tech debt", item 1.

## 3. ~~DX: spec-type inference for single-segment tuple-struct ctors~~ — ✅ DONE (2026-07-08)

`spec_type_of` (`r2e-macros/src/codegen/decorators.rs`) now accepts a
single-segment uppercase call as a tuple-struct constructor:
`#[guard(RequireApiKey("x-api-key"))]` infers spec type `RequireApiKey`
directly (lowercase single-segment calls — free functions — still require
the `MyGuard = expr` escape hatch). Compile-pass test:
`guard_tuple_struct_ctor.rs`; runtime coverage in
`r2e-core/tests/decorator_bean.rs`.

## 4. ~~`#[derive(DecoratorBean)]` — kill the spec/product boilerplate~~ — ✅ DONE (2026-07-08)

Named `DecoratorBean` (user decision — one derive serves guards AND
interceptors; the underlying trait is `DecoratorSpec`). Site syntax (user
decision): a generated associated constructor, `Name::spec(<plain fields in
declaration order>)`, returning a **hidden** companion spec
`__R2eSpec_<Name>` — nothing leaks into the user's API surface.

```rust
#[derive(DecoratorBean)]
pub struct DbAuditLog {
    #[inject] pool: SqlitePool,             // bean graph (compile-checked)
    #[config("audit.channel")] chan: String,// R2eConfig (adds R2eConfig dep)
    tag: &'static str,                      // plain = config, ctor arg
}
// site: #[intercept(DbAuditLog::spec("t1"))]
```

Mechanics (`r2e-macros/src/decorator_bean_derive.rs`): the derive emits the
hidden spec (plain fields), `Name::spec(...)`, the real `DecoratorSpec` impl
on the spec (`Product = Name`), and an **identity `DecoratorSpec` impl on
`Name` itself** carrying the same `Deps` — that impl is what the
controller's dep fold reads (`spec_type_of` extracts `Name` from
`Name::spec(...)`) and what keeps the `#[guard(Name = prebuilt)]` escape
hatch working. The codegen now emits sites through
`r2e_core::decorator::build_decorator::<S, Named>` (`S` inferred from the
expression, `Named` from the leading path) whose
`S: DecoratorSpec<Product = Named::Product, Deps = Named::Deps>` equality
bounds guarantee the fold covers exactly what `build` pulls — the
compile-check invariant survives the spec/expression split. Unsupported
(clear errors): enums, tuple structs, generics.

Tests: `r2e-core/tests/decorator_bean.rs` (e2e: inject+config+plain guard &
interceptor through a real controller), compile-fail
`decorator_bean_missing_dep.rs` (same `Contains` diagnostic as hand-written
specs) + `decorator_bean_unsupported.rs`. example-app's
`DbAuditLog`/`DbAuditLogReady` pair collapsed to one derived struct.

## 5. Wiring-time `BeanContext` for scheduled/gRPC interceptors

**Problem.** `#[intercept(...)]` on `#[scheduled]` methods and gRPC methods
uses the expression directly as the interceptor (no wiring-time context in
`wrapping.rs` / `grpc_codegen/trait_impl.rs`), so only `SelfBuilt`
decorators work there; config specs (`Cache`, a DB audit interceptor…)
don't compile.

**Direction.** Scheduled tasks are collected at registration
(`scheduled_tasks_boxed`), where the retained context exists — thread it
through so scheduled wrapping can `DecoratorSpec::build` like routes do.
Same idea for gRPC services (they construct from the context by type
already). Removes the last decorator asymmetry.

**Origin:** "Known gaps" in `plan-guards-as-beans.md`.

## 6. (Opportunistic) Controller-level interceptor instance sharing

Today a controller-level `#[intercept]` site is instantiated **once per
route method** (each `__R2eDeco_<C>_<m>` holds its own product; state
persists across requests but is per-method). If a shared-per-controller
instance is ever needed: emit one controller-level set, referenced by the
per-method sets. No built-in needs it; documented behavior in
`guards-interceptors.md`. Low priority.

## 7. ~~Qualifiers / named beans~~ — REJECTED (user decision, 2026-07-08)

The graph stays **typed-indexed with newtypes** as the way to distinguish
same-shaped beans (e.g. `ReadPool(PgPool)` / `WritePool(PgPool)`). The user
explicitly prefers newtypes — they keep things clean and explicit. Do NOT
propose a `Qualified<T, Name>` / `#[inject(name = "...")]` system. The
runtime `DuplicateBean` error and the Phase 5 module rule (same-typed
private beans across modules ⇒ newtype) remain the intended design.

## 8. (Watch) Compile-time scalability of the type-level machinery

`AllSatisfied` is O(R×P) in trait resolution, HLists are linear, and
>~127 registrations need `#![recursion_limit = "512"]` (`r2e doctor`
warns). No action needed now; if real apps grow, measure build times before
designing anything (balanced type-lists, per-module partitioning of `P`).

Related (recorded 2026-07-08, item-1 review): dep lists are NOT deduped at
the bean level. `controller_deps_fold` dedups by *spec type* only, so a bean
read by several specs — or by both a spec and an `#[inject]` field — appears
multiple times in `ControllerDeps::Deps`, and `ControllerDepsList` folds
those duplicates into the module-scope check (one extra linear
`InModuleScope` walk per duplicate; longer type-lists count toward the
recursion-depth budget above). The macro cannot dedup by bean —
`DecoratorSpec::Deps` is an opaque associated type at expansion time; a
type-level dedup trait would likely cost more than it saves. Harmless today;
revisit only together with this item if build times ever matter.

## 9. (Watch) Bean disposal hooks

`PostConstruct` exists; app-level `on_stop` exists; there is no per-bean
`@PreDestroy` equivalent (close a pool, flush a buffer on shutdown, in
reverse dependency order). Independent, additive extension of the bean
graph if the need materializes.
