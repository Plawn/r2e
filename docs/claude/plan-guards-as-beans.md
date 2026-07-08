# Plan — Guards & Interceptors as Graph-Resolved Decorators (Phase 6)

Status: **COMPLETE** (6a spike → 6b core traits → 6c codegen → 6d built-ins/examples → 6e docs+diagnostics; landed 2026-07-08). The "Known gaps" section below records the accepted limitations.

## Spike decisions (6a, `r2e-core/tests/decorator_spike.rs`, 2026-07-08)

- **(d1) Coherence — blanket + marker WORKS, including cross-crate.** The
  expected E0119 does not happen: modern negative coherence accepts
  `impl<D: SelfBuilt + Send + Sync + 'static> DecoratorSpec for D` alongside
  manual config-type impls (`RateLimitCfg`, `AuditCfg`) even when those live
  in a downstream crate (validated with a two-crate scratch probe) — a local
  type with no `SelfBuilt` impl cannot gain one elsewhere (orphan rules).
  Self-contained decorators opt in with one line
  (`impl SelfBuilt for MyGuard {}`); no derive strictly needed (a
  `#[derive(GuardBean)]` for dep-carrying user guards remains a 6b
  nice-to-have). Caveat: a type cannot be both `SelfBuilt` and a manual
  spec — the compiler rejects the ambiguity, which is what we want.
- **(d2) Decorators build from the `BeanContext`, not the state.** Keeps
  module-private guard deps working exactly like private core deps (no
  repeat of the bean-backed-extractor asymmetry from Phase 5). Consequence:
  `Controller::routes` gains a `ctx: &BeanContext` parameter (breaking;
  registration holds the retained context). Validated: spec build is fully
  independent of the provision list `P`.
- **(d3) Runtime shape: one `Arc` of the site-decorator set per route,
  captured by the handler closure** (axum handlers must be `Clone`).
  Per-request cost = one Arc clone + monomorphized `check`/`around` calls on
  prebuilt fields. Build-once verified (spec `build` ran exactly once across
  4 requests); guard declaration order and short-circuit-before-interceptors
  verified end to end through the real `register_controller()`.
- **(d4) Deps fold via `TAppend` projections needs no extra bounds** — all
  site lists are concrete, so `<A as TAppend<B>>::Output` normalizes inside
  the generated `Controller::Deps` directly.
- **(d5) Diagnostic quality is already right.** With a guard bean missing,
  the existing `Contains` `on_unimplemented` fires at the
  `register_controller::<SpikeController>()` call site:
  `` type `SpikeRegistry` was not provided to the AppBuilder — missing
  `.provide::<SpikeRegistry>()` or `.register::<SpikeRegistry>()` `` — same
  UX as a missing `#[inject]` bean, no new diagnostic traits required at the
  app level.

Resolves Phase 4 tech-debt item 2 (`di-builder-refactor.md` § "Phase 4 tech
debt"): guard/interceptor bean deps are runtime-checked, and both guards and
interceptors are re-constructed on every request. This plan moves them into
the bean graph — Quarkus-CDI-style, where interceptors and security checks
*are* beans — with compile-time dep checking and once-at-registration
construction.

## Current state (verified 2026-07-08)

- **Guards and interceptors are two disjoint mechanisms.** Guards are inline
  `Guard::check(&#expr, &state, &ctx)` statements emitted before the handler
  body (`codegen/handlers.rs::generate_guard_checks`); their presence forces
  the handler into "Case 3" (returns `Response`). Interceptors are a
  monomorphized `Interceptor::around` nesting that preserves the raw return
  type. Guards are NOT built on top of interceptors, and should not be: they
  see request metadata (`GuardContext`: headers, uri, path params, identity)
  and short-circuit with `Response`; interceptors see none of that and must
  preserve `R`. `#[roles]` already desugars to a `RolesGuard { .. }` guard
  expression (`extract/route.rs::roles_guard_expr`) — one guard path.
- **Both are constructed per request.** The guard expression and every
  interceptor expression (`let __interceptor = #intercept_expr;`) are
  evaluated inside the handler/closure on every call.
- **Bean access is dynamic and per request.** `RateLimitGuard` /
  `PreAuthRateLimitGuard` do `state.bean::<RateLimitRegistry>()`
  (`BeanLookup`, a monomorphized TypeId-compare chain) on every check; a
  missing bean is a per-request 500 (`registry_missing_response()`). `Cache` /
  `CacheInvalidate` bypass the graph entirely via the global
  `r2e_cache::cache_backend()`.
- **The macro cannot name expression types.** `#[guard(expr)]` takes an
  arbitrary expression; every witness-threading bound (`AllSatisfied`, HList
  field types) needs a nameable type. This is the recorded blocker — not
  E0207.
- `RateLimit::per_user(5, 60)` returns `RateLimitGuard` directly;
  `RateLimit::global/per_ip` return `PreAuthRateLimitGuard`. All built-in
  interceptor builders (`Logged::info()`, `Cache::ttl(30).group("x")`,
  `Counted::new(..)`, …) return `Self`.

## Goals

1. **Compile-time dep checking**: a guard/interceptor that needs a bean the
   app didn't provide must fail at `register_controller` with a diagnostic
   naming the type — same UX as `#[inject]`, and subject to Phase 5 module
   encapsulation for free.
2. **Zero per-request cost beyond the check itself**: no per-request
   construction, no `BeanLookup` TypeId chain, no global lookups. Deps are
   plain fields on a prebuilt instance; calls stay fully monomorphized.
3. **Code cleanup**: drop the `S` state parameter from the request-time
   traits, shrink the handler case matrix, delete the per-request
   missing-bean 500 path and the `cache_backend()` global.

Breaking changes are allowed (pre-production). The `Guard::startup_check`
idea from the tech-debt entry is **superseded** by this plan — skip it.

## Design

### 1. Request-time traits lose `S` (breaking)

Deps become fields, injected at construction. The state parameter — the only
reason guards/interceptors were generic over `S` — disappears:

```rust
pub trait Guard<I: Identity>: Send + Sync {
    fn check(&self, ctx: &GuardContext<'_, I>)
        -> impl Future<Output = Result<(), Response>> + Send;
}

pub trait PreAuthGuard: Send + Sync {
    fn check(&self, ctx: &PreAuthGuardContext<'_>)
        -> impl Future<Output = Result<(), Response>> + Send;
}

pub trait Interceptor<R>: Send + Sync {
    fn around<F, Fut>(&self, ctx: InterceptorContext<'_>, next: F)
        -> impl Future<Output = R> + Send
    where F: FnOnce() -> Fut + Send, Fut: Future<Output = R> + Send;
}
```

`InterceptorContext` loses its `state` field (and its `S` param). Request
path after this: `Arc` deref + monomorphized calls on prebuilt fields.
`BeanLookup` remains only for `ManagedResource`.

### 2. `DecoratorSpec` — the construction/dep contract

One trait shared by guards and interceptors (working name; could split into
`GuardSpec`/`InterceptorSpec` aliases if diagnostics benefit):

```rust
pub trait DecoratorSpec: Sized {
    /// The guard or interceptor this spec builds.
    type Product: Send + Sync + 'static;
    /// Beans required at build time — folded into the controller's Deps.
    type Deps: TypeList;
    /// Called once at registration, with the resolved graph.
    fn build(self, ctx: &BeanContext) -> Self::Product;
}
```

- **Self-contained decorators** (`RolesGuard`, `Logged`, `Timed`, `Counted`,
  `MetricTimed`, user unit guards): `Product = Self`, `Deps = TNil`,
  `build = |self, _| self`, via the `SelfBuilt` marker + blanket impl
  (spike decision d1 — negative coherence makes the blanket legal, even
  cross-crate). One line per decorator: `impl SelfBuilt for MyGuard {}`.
- **Bean-reading decorators**: the *expression type* is a pure config value;
  `build` pulls beans once. E.g. `RateLimit` becomes the spec
  (`per_user(5,60)` returns `RateLimit`, not the guard) with
  `Product = RateLimitGuard`, `Deps = TCons<RateLimitRegistry, TNil>`;
  `build` moves the registry into the guard. `Cache`/`CacheInvalidate` gain
  `Deps = <the CacheStore bean>` and drop the `cache_backend()` global.
- **User guards with deps**: symmetric with controllers via a field-attr
  derive:

  ```rust
  #[derive(GuardBean)]           // generates the DecoratorSpec impl:
  pub struct AuditGuard {        //   Deps from #[inject] fields,
      #[inject] pool: PgPool,    //   build via ctx.get::<PgPool>()
  }
  ```

### 3. Syntax — unchanged in the common case

The macro extracts the **leading type path** of the attribute expression
syntactically and takes it as the spec type:

| Attribute | Spec type |
|---|---|
| `#[guard(RateLimit::per_user(5, 60))]` | `RateLimit` |
| `#[guard(MyGuard)]` | `MyGuard` |
| `#[intercept(Cache::ttl(30).group("x"))]` | `Cache` |
| `#[intercept(Logged::info())]` | `Logged` |

New contract, checked by the compiler at registration: **the expression must
evaluate to the named spec type** (`let __spec: #SpecTy = #expr;`). This
holds today for every built-in except `RateLimit::*` (fixed by making
`RateLimit` the config value, § 2). Method chains work because builders
return `Self`. Escape hatch when the expression has no usable leading path
(free function, variable): `#[guard(MyGuard = make_guard())]` /
`#[intercept(Audit = build_audit())]`.

`#[roles(..)]` keeps desugaring to `RolesGuard { .. }` — it rides the same
path unchanged.

### 4. Codegen — decorators become hidden core fields

`#[controller]`/`#[routes]` liaison gains one piece: the decorator set is
appended to the generated **core struct** as hidden fields (one per site,
`__r2e_g_<method>_<idx>` / `__r2e_i_<method>_<idx>`; controller-level
`#[intercept]` sites get one shared field each):

- The core's generated `ContextConstruct::from_context` builds each field:
  `<#SpecTy as DecoratorSpec>::build(#expr, ctx)` — **once, at
  `register_controller`**.
- The core's `Deps` = inject fields + `R2eConfig` + fold of every site's
  `<#SpecTy as DecoratorSpec>::Deps`. The existing `AllSatisfied` check at
  `register_controller` then rejects a missing guard/interceptor bean at
  compile time, naming the type. Phase 5 `InModuleScope` checks apply
  unchanged.
- Controller-level `#[intercept]` sites are re-evaluated **per route
  method** (each method's decorator set holds its own product instance,
  built once at registration). As-landed semantics, confirmed at the review
  gate: a stateful controller-level interceptor gets per-method state that
  now persists across requests (previously it was rebuilt per request). No
  built-in interceptor holds per-instance mutable state, so no behavior
  change for built-ins.
- Since `#[routes]` (not `#[controller]`) sees the sites, the decorator
  values cannot live on the core struct emitted by `#[controller]`.
  **Spike decision d2/d3**: no runtime side struct at all —
  `Controller::routes` gains `ctx: &BeanContext` (breaking) and builds each
  site's decorator there, wrapping the route's set in one `Arc` moved into
  the handler closure. The side struct survives only as a *type-level*
  device: `#[routes]` emits `Controller::Deps` as the `TAppend` fold of
  `<Core as ContextConstruct>::Deps` ++ every site's
  `<Spec as DecoratorSpec>::Deps` (no extra bounds needed — d4).

Consequences in `codegen/handlers.rs`:

- `needs_state` shrinks to `has_managed` — guards and interceptors no longer
  need `__state: __R2eS`, so the case matrix collapses (guards no longer
  drag handlers into the state-generic shape by themselves).
- `generate_guard_checks` references fields instead of evaluating
  expressions; `wrap_with_handler_interceptors` drops the per-request
  `let __interceptor = …`.
- Guard config expressions may reference `path::<param>` descriptors
  (`PathParam` consts). Today the `path` module is generated inside the
  handler; it moves to impl scope (one `mod __r2e_path_<method>` per route
  method) so spec expressions can reference it at construction time.
  `PathParam::new` is already `const`.
- `PreAuthGuard` sites get the same treatment; the prebuilt instance is
  moved into the middleware closure at wiring time
  (`controller_impl.rs` pre-guard emission).

### 5. Deletions

- `Guard<S, I>` / `PreAuthGuard<S>` / `Interceptor<R, S>` state params and
  every `S: BeanLookup` bound on guard/interceptor impls.
- `registry_missing_response()` and the per-request missing-bean 500 path in
  `r2e-rate-limit` (and the equivalent in `r2e-openfga`).
- `r2e_cache::cache_backend()` global (Cache store becomes a graph dep).
- The `Guard::startup_check` tech-debt idea (superseded).

## Migration (breaking)

| Crate | Change |
|---|---|
| `r2e-core` | New traits (§1), `DecoratorSpec`, `InterceptorContext` without state |
| `r2e-macros` | Spec-type extraction, side struct + Deps fold, path-module hoist, `Type = expr` escape hatch, derives |
| `r2e-rate-limit` | `RateLimit` becomes the spec/config value; guards hold `RateLimitRegistry` as a field |
| `r2e-openfga` | `FgaGuard` spec with FGA client in `Deps` |
| `r2e-security` | `RolesGuard`: `DecoratorSpec` with `Product = Self` |
| `r2e-utils` | All interceptors: `Product = Self`; `Cache`/`CacheInvalidate` gain store dep |
| `example-app`, CLI templates, `r2e-test`, book | Custom guards move `state.bean::<T>()` → injected fields + `#[derive(GuardBean)]` |

## Known gaps (recorded during 6b/6c implementation)

- ~~**Module controllers' decorator deps are not compile-checked.**~~
  **CLOSED (2026-07-08, di-next-steps item 1).** `#[routes]` now emits a
  state-independent `ControllerDeps` carrier (`r2e-core/src/controller.rs`)
  holding the full fold (`ContextConstruct::Deps` ++ decorator deps);
  `Controller::Deps` points at it and `ControllerDepsList` folds it, so the
  module-scope check covers guards/interceptors. Out-of-scope decorator deps
  are now a compile error at `register_module` (trybuild:
  `module_controller_guard_dep_not_in_scope.rs`).
- **Scheduled-method and gRPC interceptors bypass `DecoratorSpec`.** They
  run outside the handler path with no wiring-time `BeanContext`, so the
  `#[intercept(expr)]` expression is used directly as the interceptor —
  fine for `SelfBuilt` decorators (`Logged`, `Timed`, …), a compile error
  for config specs like `Cache` (which never made sense there anyway).
- **Pre-existing bug fixed in passing**: SSE/WS methods with `#[pre_guard]`
  used to be filtered out of normal registration but never registered by the
  pre-auth path — the route silently vanished. They now go through the same
  pre-auth middleware wiring as HTTP routes.

## Open questions — resolved by the 6a spike

1. **Coherence layout** → d1: `SelfBuilt` marker + blanket, works
   cross-crate. `on_unimplemented` on `DecoratorSpec` should suggest both
   routes ("implement `SelfBuilt` for a self-contained guard/interceptor,
   or implement `DecoratorSpec` for a config type").
2. **Deps fold** → d4/d5: `TAppend` projections normalize without bounds;
   `AllSatisfied`/`Contains` diagnostics unchanged and already name the
   missing type at the registration call site.
3. **RPITIT `Send` inference** with the no-`S` traits → holds end to end
   through axum's `Handler` bounds (validated in the spike router).
4. **Identity generic** → guards stored as concrete types; `I` appears only
   on `check`, bound at the emission site via the meta module as today.

## Phasing

- **6a — spike**: side struct + Deps fold + spec-type extraction on one
  controller in a scratch test; validate coherence layout and diagnostics.
- **6b — core**: new traits in `r2e-core` (`Guard<I>`, `PreAuthGuard`,
  `Interceptor<R>`, `DecoratorSpec`), derives in `r2e-macros`.
- **6c — codegen**: `#[routes]` emits the decorator struct, hoisted path
  modules, field-referencing checks/wraps; `needs_state` collapse.
- **6d — migrate built-ins** (rate-limit, openfga, security, utils) +
  example-app + CLI templates.
- **6e — docs + trybuild**: missing-dep diagnostic tests (app-level and
  module-encapsulation), update `guards-interceptors.md`, book.

Each phase ends with the quality-review gate (same discipline as Phases 4–5).
