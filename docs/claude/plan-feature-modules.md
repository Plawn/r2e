# Plan — Feature Modules (Closed Subgraphs)

> Implementation plan for a **future session**. Not started. **Depends on**
> `docs/claude/plan-controllers-as-beans.md` (Direction A).

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
    type Provides;     // TCons of bean types the module registers
    type Exports;      // TCons ⊆ Provides made visible to the app-global P
    type Imports;      // TCons of bean types required from outside the module
    type Controllers;  // tuple of Controller types (reuse ControllerTuple, Phase 1f)
    fn register(builder) -> builder;  // registers providers + controllers
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

If controllers resolve from the graph/context (Direction A), a module registers
beans **and** controllers in one graph pass — no user-visible two-phase split,
one `.register_module` call. Without Direction A, a module must split into a
pre-state half (providers) + a typed half (controllers) → two calls. So land
`plan-controllers-as-beans.md` first.

## API / codegen

- A `#[module]` attribute macro (or a builder-style declaration) that, from a
  listing of providers / controllers / imports / exports, generates the
  `FeatureModule` impl: `Provides`/`Exports`/`Imports` as `TCons` lists (via
  `build_tcons_type`, `type_list_gen.rs`) and a `register` body chaining
  `.register::<Provider>()` + `.register_controllers::<Controllers>()`.
- Module-local encapsulation check emitted alongside.

## Known hard issue to resolve in the spike

`BeanRegistry` keys beans by **global `TypeId`** (`beans.rs`). Compile-time
encapsulation controls **visibility**, not runtime keying: two modules that each
have a *private* provider of the **same concrete type** would still **collide**
at runtime (last-wins / `DuplicateBean`). Options: (a) accept + document the
limitation (private beans must still be distinct types — use newtypes); (b) a
per-module `TypeId` namespacing scheme in the registry (larger change). Decide in
the spike.

## Other open questions

- Exact type-level encoding of "internal deps satisfied by `Provides ∪ Imports`"
  and "only `Exports` leak to global `P`".
- Interaction of module boundaries with lazy beans, producers, `#[post_construct]`.
- Nested modules / a module importing another module's exports.
- Ergonomics: `#[module]` declaration form vs a builder DSL.

## Scope (breaking-additive)

New `#[module]` macro (`r2e-macros`), `FeatureModule` trait + `register_module`
(`r2e-core` `builder.rs` + `type_list.rs`), module-local encapsulation check.
Depends on the controllers-as-beans plan.
