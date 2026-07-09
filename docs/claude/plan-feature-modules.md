# Plan — Feature Modules (Closed Subgraphs)

> **✅ DONE** (Phase 5, landed 2026-07-08 on `refactor/di-builder-dx-ct`).
> Implemented as designed below — spike decisions (see "Spike results") held
> through implementation with no deviations. Landed surface: `FeatureModule`
> + `BeanList` + encapsulation traits (`r2e-core/src/module.rs`),
> `register_module` (`RegisterModule` extension trait) + `Mods` builder
> param + `build_state` module fold (`r2e-core/src/builder/`), `#[module]`
> macro (`r2e-macros/src/module_attr.rs`), tests
> (`r2e-core/tests/module.rs`, trybuild cases in `r2e-compile-tests`),
> example-app `UserModule`. **Depends on**
> `docs/claude/plan-controllers-as-beans.md` (Direction A) — which has now
> **landed** (Phase 4, A3): controllers resolve from the graph/context and the
> state is the inferred HList of the provision list `P`. The single-pass
> assumption below therefore holds; this plan is unblocked.

## Context / motivation

Add Spring/NestJS-style modules: a unit bundling **providers (beans) +
controllers + imports/exports**, registered with one call
`.register_module::<UserModule>()`, so feature-sets drop into an app cleanly.
Crucially, R2E can enforce **compile-time encapsulation** that Spring/NestJS
cannot: a module may depend only on its declared imports, and only its exports
are visible outside — anything else is a compile error.

## What already exists (do not duplicate)

The plugin system is R2E's current extension mechanism
(`r2e-core/src/plugin.rs`): a `RawPreStatePlugin` (NoState phase) can register
beans and grow `P`/`R`; a `Plugin` (typed phase) can register controllers/layers
(as `r2e-openapi` does for `/docs`). A "module" today = a plugin (or a pair, one
per phase). The gap is a **single, first-class, encapsulated** bundle — this plan.

## Model (mirrors `@Module`)

```rust
trait FeatureModule {
    type Providers;    // TCons of Registrable provider types (beans/producers)
    type Controllers;  // tuple of Controller types (reuse ControllerTuple, Phase 1f)
    type Exports;      // TCons of bean types ⊆ Provided, made visible to the app-global P
    type Imports;      // TCons of bean types required from outside the module
    // no register body — registration is derived from Providers (see spike results)
}
```

`.register_module::<UserModule>()` registers the module's beans (growing `P` by
`Exports`, `R` by `Imports`) and its controllers (via the 1f tuple mechanism).

## Closed-subgraph encapsulation (the differentiator, compile-time)

- A module's providers may depend only on `{module-internal providers} ∪ Imports`.
  Enforce with a **module-local** `AllSatisfied`-style check over
  `Provides ∪ Imports` (reuse `Contains`/`AllSatisfied`, `type_list.rs`).
- Only `Exports` are appended to the app-global provision list `P`;
  internal-only providers stay private (not visible to other modules).
- Result: depending on another module's private bean = **compile error** — real
  encapsulation on top of the single global topological sort.

## Two-phase reconciliation (why this depends on Direction A)

Because controllers now resolve from the graph/context (Direction A, landed in
Phase 4), a module can register beans **and** controllers in one graph pass — no
user-visible two-phase split, one `.register_module` call. (Before Direction A a
module would have had to split into a pre-state half (providers) + a typed half
(controllers) → two calls; that constraint is now gone.)

## API / codegen

- A `#[module]` attribute macro that, from a listing of providers /
  controllers / imports / exports, generates the `FeatureModule` impl:
  `Providers`/`Exports`/`Imports` as `TCons` lists (via `build_tcons_type`,
  `type_list_gen.rs`) and `Controllers` as a tuple. No register body — the
  `BeanList` fold derives registration (see spike results).
- Module-local encapsulation check enforced by `register_module`'s bounds.

## Spike results (2026-07-08) — decisions locked

Validated end-to-end by `r2e-core/tests/module_spike.rs` (local prototypes of
the traits below over the public Phase 4 machinery; 5a moves them into
r2e-core proper). All witness inference works at real call sites, including
extraction markers (`#[inject(request)]`) through the module fold.

### (a) TypeId collision for same-typed private beans → **newtype requirement**

Decision: **accept + document** (option a). Private beans of the same concrete
type across modules must be distinct newtypes. Rationale: the entire Phase 4
model is by-type — `ctx.get::<T>()` in generated `ContextConstruct` impls,
`BeanLookup`'s TypeId chain, `HasBean` slots — and generated code has no
module identity to key a namespace with; registry namespacing would ripple
through every resolution path for a case Rust idiom already answers with
newtypes. A collision is caught **loudly at startup** by the existing runtime
`DuplicateBean` check (first thing `build_state` does).

### (b) Type-level encoding — declarative trait + `BeanList` fold

`FeatureModule` is **purely declarative** (no `register` body — an impl
cannot misdeclare its deps):

```rust
trait FeatureModule {
    type Providers;    // TCons list of Registrable types (beans/producers)
    type Controllers;  // tuple of controller types (or ())
    type Exports;      // TCons list of bean types leaked to global P
    type Imports;      // TCons list of bean types required from outside
}
```

A `BeanList` fold over `Providers` derives everything:
`type Provided` (each `Registrable::Provided`), `type Deps` (concatenation of
each `Registrable::Deps`), and `fn register_into(&mut BeanRegistry)`. Lazy
beans, producers, and `#[post_construct]` need no special handling — they all
flow through `Registrable::register_into` (answers that open question).

With `LocalScope<M> = Provided ++ Imports`, `register_module` checks at
compile time:

1. `<Providers as BeanList>::Deps ⊆ LocalScope` — provider encapsulation;
2. `Exports ⊆ Provided` — can't export what you don't provide;
3. controller deps ⊆ `LocalScope` — via **`ControllerDeps::Deps`** (crucially
   state-independent, so checkable in the NoState phase before the state type
   exists), folded over the controller tuple. Originally this walked
   `ContextConstruct::Deps` (core `#[inject]` deps only); since the
   post-Phase-6 `ControllerDeps` carrier it is the full list including
   guard/interceptor `DecoratorSpec::Deps`.

**Exports-only leakage**: `P` grows by `Exports` only; `R` grows by `Imports`
only. **Key discovery**: provider-internal deps must NOT be appended to the
global `R` (unlike `.register()`) — they are consumed by check 1; appending
them would fail the global `AllSatisfied` at `build_state`, since private
beans are absent from `P`. Imports remain in `R` and are checked against the
final `P` by the existing machinery.

The spike confirmed reusing `AllSatisfied`/`Contains` for checks 1–3 produces
**misleading diagnostics** ("type X was not provided to the AppBuilder ... add
`.provide(value)`") when the real fix is "add a provider/import to the module"
or "export only provided types". 5b introduces dedicated trait pairs with
module-targeted `#[diagnostic::on_unimplemented]` messages, mirroring the
`Contains`/`AllSatisfied` impl shape.

### (c) Registration placement — NoState beans + deferred controller fold

`.register_module::<M>()` is a **NoState-phase** call:

- Runtime: `BeanList::register_into(&mut registry)` registers all providers
  (private ones included — the single global topological sort constructs
  them); one phantom rewrite sets `P += Exports`, `R += Imports`.
- Controllers are **deferred to the typed phase**: `AppBuilder` gains a 4th
  phantom param `Mods` (default `TNil`, so the typed phase and existing code
  are untouched); `register_module` pushes `M` onto `Mods`; `build_state()`
  runs a `ModuleFold` over `Mods` after materializing the state. The fold
  registers module controllers through an **unchecked** variant of the
  registration backend — no global `AllSatisfied` on `Deps` (replaced by the
  module-local check 3); cores construct from the retained
  `Arc<BeanContext>`, where private beans exist.
- Ripple (validated small): `RawPreStatePlugin::install` gains the `Mods`
  param (only `r2e-scheduler` hand-implements it); `with_updated_types`
  preserves `Mods`; `with_state` is constrained to `Mods = TNil` (module
  controllers need a real bean context).
- Bean-backed request extraction on module controllers (e.g.
  `AuthenticatedUser` needs `Arc<JwtClaimsValidator>` in the **state**) works
  when the bean is imported or globally provided — imports land in `P`, so
  declaring it an import suffices. A missing one is a `HasBean` compile error
  at `build_state`.

## Remaining open questions (deferred, not blockers)

- Nested modules / module-imports-module composition: v1 supports a module
  importing another module's **exported bean types** (plain `Imports`); a
  first-class "imports = [OtherModule]" form is future work.
- `#[module]` ergonomics (5c): attribute-macro declaration form
  (providers/controllers/imports/exports lists) generating the
  `FeatureModule` impl.

## Scope (breaking-additive)

New `#[module]` macro (`r2e-macros`), `FeatureModule` trait + `BeanList` +
`register_module` + `Mods` builder param (`r2e-core` `builder/`, `module.rs`),
module-local encapsulation checks with dedicated diagnostics + trybuild
coverage. Breaking (accepted, pre-production): `RawPreStatePlugin::install`
signature gains the `Mods` type param.

## Sub-items

- **5a** — `FeatureModule` trait + `BeanList` + `Mods` threading +
  `register_module` + `build_state` module fold (hand-written module impls).
- **5b** — dedicated encapsulation-check traits with module-targeted
  diagnostics; trybuild coverage in `r2e-compile-tests`.
- **5c** — `#[module]` macro generating the `FeatureModule` impl.
- **5d** — example-app vertical-slice migration + docs + phase close.
