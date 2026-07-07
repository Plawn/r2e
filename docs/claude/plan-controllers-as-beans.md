# Plan — Controllers as Graph-Resolved Beans (Direction A)

> Implementation plan for a **future session**. Not started. Builds on the
> completed Phase 1 (`docs/claude/di-builder-refactor.md`).

## Context / motivation

Phase 1 cleaned up the builder DX. A deeper structural friction remains
(audit friction #1): the user must hand-write a `BeanState` struct (e.g.
`Services`) that aggregates every bean a controller injects, because controller
cores are built from that typed state **by field name**
(`r2e-macros/src/controller_codegen.rs:311`: `__state.#field_name.clone()`).
Beans already form a resolved dependency graph (`BeanContext`), and a
controller's `#[inject]` fields **are** beans. This plan makes controller cores
resolvable **from the graph by type**, reducing or eliminating the manual state
struct — the same question as "why aren't controllers treated like beans?".

## Baseline mechanics (what exists today)

- `#[derive(BeanState)]` on `Services` generates `FromRef<Services>` per field
  type + `BeanState::from_context` (`r2e-macros/src/bean_state_derive.rs`).
- `register_controller` (`r2e-core/src/builder.rs`) builds the core **once**:
  `Arc::new(C::from_state(&state))`, then wires routes with that `Arc` — the
  `Controller::routes(&state, __core: Arc<Self>)` signature
  (`r2e-macros/src/codegen/controller_impl.rs:67`).
- `StatefulConstruct::from_state` pulls `#[inject]` fields **by name**:
  `__state.user_service.clone()` — requires `Services` to have identically-named
  fields (this is *why* the struct is manual).
- Per request: `Arc`-clone of the core + `bind_request` (FromRequestParts for
  request-scoped fields) — `handlers.rs:1295`. Request-scoped extractors
  (`AuthenticatedUser<S>`) pull app deps via `FromRef<S>` — a direct
  monomorphized field clone, **no lookup** (`r2e-security/src/extractor.rs:110`).

## The core change

Replace `StatefulConstruct<S>::from_state(&S)` with a context constructor:

```rust
impl ContextConstruct for UserController {          // replaces StatefulConstruct<S>
    fn from_context(ctx: &BeanContext) -> Self {
        Self {
            user_service: ctx.get::<UserService>(),          // by TYPE, from the graph
            greeting:     ctx.get::<R2eConfig>().get::<String>("app.greeting")
                              .unwrap_or_else(|_| panic!(/* … */)),
        }
    }
}
```

`ctx.get::<T>()` resolves by type (no name-matching), so the controller draws
from the same graph as beans and `Services` is no longer needed to name-match.

## THE key design decision: what is the axum state `S`?

Request-scoped extractors stay bounded `Arc<JwtClaimsValidator>: FromRef<S>` and
call `from_ref(state)` **per request** (`extractor.rs:110`). So `S` must still
supply app deps to these extractors. Two options:

**Option A1 — Context as state (`S = Arc<BeanContext>`), no user struct.**
- Provide a blanket `FromRef<Arc<BeanContext>>` (via `ctx.get`) so request
  extractors resolve their deps.
- PRO: eliminates the manual `Services` struct entirely.
- CON (per-request, honest): request extractors do `ctx.get::<Arc<Validator>>()`
  = a TypeId `HashMap` lookup + clone **per request**, instead of a direct field
  clone. ~tens of ns, dwarfed by JWT validation, but **not literally zero**.
  Core construction stays one-time.
- CON: `BeanContext` must be retained (Arc'd) as state — mind the
  `BeanContext::clone` overlay footgun (use `Arc<BeanContext>`, never clone the
  inner context).

**Option A2 — Keep a typed state struct, resolve controller cores from context.**
- Controller cores built via `from_context` at startup; request extractors keep
  `FromRef` on a typed struct (fast, no per-request lookup).
- CON: the typed struct still must exist (shrinks to only request-extractor
  deps, not all controller deps) — does not fully remove the manual struct.
- NOTE: a proc-macro **cannot** auto-generate a single app-wide state struct
  (no whole-program view across the separate `register` calls), so full
  auto-generation is infeasible — this is why R2E makes the struct explicit.

**Recommendation:** decide via a spike. A1 maximizes DX (no struct) at a small,
bounded per-request cost on secured endpoints; A2 preserves per-request perf
exactly but keeps a (smaller) struct. A1 is likely acceptable since the lookup
is negligible next to auth cost — but **measure it** before committing.

## Per-request performance (corrected, authoritative)

| Path | Baseline | This plan (A1) | This plan (A2) |
|---|---|---|---|
| Core `#[inject]` resolution | 1× startup (`from_state`) | 1× startup (`from_context`) | 1× startup |
| Request extractor app-dep access | `FromRef` direct clone (0 lookup) | `ctx.get` TypeId lookup **per req** | `FromRef` (iso) |
| Arc-core clone + `FromRequestParts` | — | unchanged | unchanged |

INVARIANT (must hold in any option): the core stays built **once** in an `Arc`;
request scope stays a `FromRequestParts` concern on the stack — **never** a graph
scope (that would reintroduce Spring/Quarkus `@RequestScoped` proxy/scope-map
overhead).

## Codegen changes

- `controller_codegen.rs`: `generate_stateful_construct` → `generate_context_construct`:
  `__state.#field.clone()` → `ctx.get::<#ty>()`; config `FromRef<S>::from_ref` →
  `ctx.get::<R2eConfig>()`.
- `builder.rs` `register_controller`: `Arc::new(C::from_state(&state))` → build
  from the retained context; retain the `BeanContext` (as `Arc`) out of
  `build_state` instead of dropping it after assembling the state.
- If A1: generate a blanket `impl<T: Clone + 'static> FromRef<Arc<BeanContext>> for T`
  and set the axum state to `Arc<BeanContext>`.

## Compile-time guarantee to preserve

A controller injecting `T` that is not in the graph must remain a **compile
error** (today via `FromRef<S>` / field presence). With A1, thread each
controller's inject types into the requirement list `R` (extend the
`Registrable` / `ControllerTuple` machinery to expose controller `Deps`) so the
existing `AllSatisfied<P>` check at `build_state` still catches a missing dep.

## Interaction with Phase 1 (must reconcile)

- **1b** (`BeanState::Requires`, `build_state!`, `AllSatisfied`): with A1 the user
  `Services` struct + `#[derive(BeanState)]` becomes **optional/internal**. The
  `build_state!` macro + the `P`/`AllSatisfied` bean-presence tracking **stay**
  (beans still must be present). `BeanState::Requires`' role (folding user field
  types into `R`) shrinks; `build_state!` may take no state type or an internal
  generated marker. → Update `di-builder-refactor.md` 1b notes when this lands.
- **1c** (`state: T`): still holds — the typed phase holds a concrete state
  (`Arc<BeanContext>` under A1). No conflict.
- **1a/1f** (`.register`, `register_controllers`): unchanged surface; the wiring
  under `register_controllers` switches to `from_context`.

## Scope (breaking)

`controller_codegen.rs`, `codegen/controller_impl.rs`, `builder.rs`
(register_controller, build_state, state field), `beans.rs` (retain/expose
context), `bean_state_derive.rs` (role change), `r2e-security` extractor
(A1 blanket FromRef). OpenAPI generation currently keys off the state — assess.

## Open questions for the spike

1. A1 vs A2 — benchmark the per-request TypeId lookup on a secured endpoint.
2. How `build_state!` reshapes without a user state struct.
3. Threading controller inject-deps into `R` for compile-time missing-dep detection.
4. OpenAPI + any plugin that assumed a typed state struct.
