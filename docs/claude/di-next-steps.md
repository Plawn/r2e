# DI — Next Improvement Axes (post-Phase 6)

Status: **BACKLOG** — recorded 2026-07-08, after Phase 6 (guards/interceptors
as graph-resolved decorators) landed. Hub: `di-builder-refactor.md` (phases
1–6 all complete). This file is the working list for the next DI session;
items are ordered by recommended priority.

## Recommended order

1 → (3 + 4 as one DX pass) → 2 → 5. Items 6/8/9 are opportunistic. Item 7 is
**rejected** (user decision — do not re-propose).

---

## 1. Module controllers' decorator deps — close the last compile-check hole

**Problem.** App-level controllers get the full `AllSatisfied` check on the
folded `Controller::Deps` (core deps ++ decorator deps). Module controllers
register through the **unchecked** backend (required: they may inject
module-private beans), and the module-local encapsulation check
(`ModuleDepsSatisfied` over `ControllerDepsList`) only walks
`ContextConstruct::Deps` — core deps, not decorator deps. A module
controller whose guard/interceptor reads a bean outside the module graph
**panics at `build_state()`** (`BeanContext::get`) instead of failing
compilation.

**Direction.** Decorator deps are state-independent (unlike
`Controller<T, W>`, which needs witnesses), so `#[routes]` can emit a
state-independent carrier — e.g. a hidden trait/assoc-type
`__R2eDecoratorDeps for <Name> { type Deps }` (the same `TAppend` fold it
already computes) — and `ControllerDepsList` folds
`ContextConstruct::Deps ++ DecoratorDeps` into the module-scope check
(`Deps ⊆ Provides ∪ Imports`). Add trybuild coverage mirroring
`module_controller_dep_not_in_scope.rs` but with the dep on a guard site.

**Origin:** "Known gaps" in `plan-guards-as-beans.md`.

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

## 3. DX: spec-type inference for single-segment tuple-struct constructors

**Problem.** `spec_type_of` (`r2e-macros/src/codegen/decorators.rs`) rejects
`#[guard(MyGuard(config))]`: for `Expr::Call` it requires ≥2 path segments
(it drops the last segment, the associated-fn name). A single-segment call
on an uppercase path is a tuple-struct constructor — unambiguously the type
itself — but today it forces the `MyGuard = MyGuard(config)` escape hatch.

**Direction.** In the `Expr::Call` arm: if the func path has exactly one
segment AND it starts uppercase, use the path as-is as the spec type.
Few-line fix + a compile-pass test.

**Origin:** Phase 6 review gate (flagged as intentional narrowing, worth
lifting).

## 4. `#[derive(GuardBean)]` — kill the spec/product boilerplate

**Problem.** A user guard with bean deps hand-writes two types (config spec
+ product) plus a manual `DecoratorSpec` impl (see `DbAuditLog` /
`DbAuditLogReady` in example-app). Self-contained guards are already
one-line (`impl SelfBuilt for X {}`), but bean-reading ones are verbose.

**Direction.** A derive symmetric with controllers:

```rust
#[derive(GuardBean)]                 // or DecoratorBean, naming TBD
struct AuditGuard {
    #[inject] pool: PgPool,
    max: u64,                        // plain fields = config, set by ctor
}
```

generating the hidden config type (non-`#[inject]` fields + constructors?)
or — simpler first cut — generating `DecoratorSpec` for a companion
builder. Design question to settle first: how the *expression* provides the
config fields while `#[inject]` fields come from the graph. Planned as a 6b
nice-to-have, never implemented.

**Origin:** `plan-guards-as-beans.md` § 2.

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

## 9. (Watch) Bean disposal hooks

`PostConstruct` exists; app-level `on_stop` exists; there is no per-bean
`@PreDestroy` equivalent (close a pool, flush a buffer on shutdown, in
reverse dependency order). Independent, additive extension of the bean
graph if the need materializes.
