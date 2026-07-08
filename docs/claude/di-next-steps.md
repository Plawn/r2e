# DI — Next Improvement Axes (post-Phase 6)

Status: **BACKLOG** — recorded 2026-07-08, after Phase 6 (guards/interceptors
as graph-resolved decorators) landed. Hub: `di-builder-refactor.md` (phases
1–6 all complete). This file is the working list for the next DI session;
items are ordered by recommended priority.

## Recommended order

~~1~~ ~~3~~ ~~4~~ ~~2~~ ~~5~~ — all done. Items 6/8/9/10 are opportunistic;
item 11 is **user-requested** (next session). Item 7 is **rejected** (user
decision — do not re-propose).

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

## 2. ~~Extraction-bridge overlap invariant~~ — ✅ DONE (2026-07-08)

Resolved as a **checked invariant**, not a constructive exclusion: both
suggested directions (sealed markers, deterministic selection) require the
negative bound "`T` does NOT implement the axum trait", which stable Rust
cannot express — any blanket re-bridge just moves the identical overlap one
level down (from `Option<T>` to `T`). Instead:

- `r2e_core::extract::assert_unambiguous_extractor<S, T, M>()` — public
  inference probe; compiles iff `T` has exactly one extraction route (one
  inferable marker) against `S`, fails with E0283 listing both impls
  otherwise. Exported from `r2e_core` root; documented as the authoring
  rule for extractor authors (probe `T` AND `Option<T>`). Known limit
  (review-gate finding, documented in the docstring): the probe only sees
  routes *reachable for `S`* — probing with a state that misses the
  extractor's backing bean silently drops the bean-backed candidate, so
  `S` must satisfy every `HasBean` bound the extractor carries.
- First-party pins: `r2e-core/tests/extract.rs` (plain-axum + a local
  bean-backed extractor), `r2e-security/tests/extractor.rs`
  (`AuthenticatedUser`), `claims_identity_macro.rs` (macro-generated
  identity).
- trybuild pins: `extractor_dual_route_probe.rs` (localized E0283 at the
  probe) + `extractor_dual_route_ambiguous.rs` (the real-world shape at
  `register_controller` — rustc names both competing impls there too, so
  the failure is diagnosable even without the probe) +
  `extractor_option_no_route.rs` (zero-route `Option<T>`: the OUTER
  `Option<T>: FromRequestPartsVia` bound surfaces, with the
  `Option`-specific note — the `OptionalFromRequestPartsVia`
  on_unimplemented itself is best-effort and rarely reachable).
- Docs: invariant section in `extract.rs` module docs,
  `#[diagnostic::on_unimplemented]` on `OptionalFromRequestPartsVia` + an
  `Option<T>`-specific note on `FromRequestPartsVia`, book troubleshooting
  row in `advanced/macro-debugging.md`.

Still true: do NOT re-add blanket bridges to `r2e-core/src/extract.rs`.

**Origin:** `di-builder-refactor.md` § "Phase 4 tech debt", item 1
(updated in place).

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

## 5. ~~Wiring-time `BeanContext` for scheduled/gRPC interceptors~~ — ✅ DONE (2026-07-08)

The last decorator asymmetry is gone — `#[intercept(...)]` on scheduled and
gRPC methods now goes through `DecoratorSpec::build` like routes, so
bean-reading specs work everywhere the attribute is accepted.

- **Scheduled**: `Controller::scheduled_tasks_boxed(state, core, ctx)` gained
  the retained `&BeanContext` (breaking for hand-written impls; call site:
  `builder/typed.rs`). `generate_scheduled_tasks` (controller_impl.rs) emits a
  hidden per-method set `__R2eSched_<C>_<fn>` **inside the fn body**, built
  once via `build_decorator`, one `Arc` clone per tick; the chain wraps the
  method call (native return type, `log_if_err` on the chain output). Body
  wrapping in `wrapping.rs` was deleted. `controller_deps_fold` now collects
  scheduled sites (and controller-level intercepts when scheduled methods
  exist), so scheduled spec deps are compile-checked at
  `register_controller`/`register_module`.
- **gRPC**: per-method sets `__R2eGrpcDeco_<C>_<fn>` live in one hidden
  container `__R2eGrpcDecos_<Name>` behind a single `Arc` field (`__decos`)
  on the `__R2eGrpc<Name>` wrapper — one ref-count bump per wrapper clone
  (tonic clones per call) regardless of method count. Built in `into_router`
  from the same context that builds the core (the wrapper's now-unused raw
  `ctx` field was dropped). Shared machinery: `generate_named_deco_items` /
  `wrap_with_deco_interceptors` in `codegen/decorators.rs` (pub(crate), used
  by handlers, scheduled, gRPC). Note gRPC deps (core AND decorators) remain
  runtime-resolved — `register_grpc_service` has no `AllSatisfied` check
  (pre-existing; a missing bean panics at registration, not per call); see
  item 10.

Behavior change: interceptor instances on scheduled/gRPC methods are now
**built once** (state persists across ticks/calls) instead of re-evaluating
the site expression per invocation — same semantics as routes.

**Direct-call interception (user decision, follow-up to the initial land):**
unlike routes, a scheduled method's interceptors must also run on DIRECT
in-code calls (e.g. an admin route calling `self.tick()`). Mechanism: every
physical core gets a hidden `__r2e_decos: DecoSlot` field
(`r2e_core::decorator::DecoSlot`, type-erased OnceLock; manual
Clone/Debug/Default keep user derives working; unit-struct controllers become
named structs — cores are no longer literal-constructible, use
`from_context`). `scheduled_tasks_boxed` fills the slot with a per-controller
container `__R2eSchedDecos_<Name>` (sets now emitted at module scope).
**Async** scheduled methods split into hidden `__r2e_sched_<fn>_inner` + a
dispatch wrapper that reads the slot and runs the chain in the body — every
call path intercepted; **sync** methods can't await a chain in their body, so
their chain stays at the task level (scheduler ticks only — documented).
Known edge: a core built via `from_context` but never registered has an
empty slot → direct calls run undecorated (test-only situation in practice).
gRPC methods keep entry-point interception (tonic dispatch) — direct calls
to gRPC methods as helpers are rare; not worth the machinery (user-approved
scope).

Tests: `example-app/tests/scheduled_test.rs`
(`scheduled_interceptor_is_built_from_the_bean_graph` — async + sync + direct
call on a registered core; `direct_call_on_unregistered_core_is_undecorated`),
`example-grpc/tests/grpc_intercept.rs` (live tonic round-trip), compile-fail
`scheduled_intercept_missing_dep.rs` (same `Contains` diagnostic as routes).
The hidden field shows up in one rustc diagnostic
(`consumer_uses_request_field.stderr`: "available fields are: …,
`__r2e_decos`") — accepted noise.

**Origin:** "Known gaps" in `plan-guards-as-beans.md` (closed there too).

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

## 10. (Opportunistic) Compile-check gRPC service deps

Recorded 2026-07-08 during the item-5 review gate. gRPC is now the only
decorator scope whose bean deps are not compile-checked:
`register_grpc_service<S: GrpcService>` (r2e-grpc/src/lib.rs) has no
`AllSatisfied` bound, so a missing bean — for the core's `#[inject]` fields
AND for `#[intercept(...)]` specs — compiles clean and panics inside
`into_router` at registration time (`BeanContext::get`). HTTP and scheduled
sites reject the same mistake at `register_controller`. If this bites, the
shape is known: a `GrpcServiceDeps` carrier emitted by `#[grpc_routes]`
(fold: `ContextConstruct::Deps` ++ Σ spec deps, mirroring `ControllerDeps`)
and an `AllSatisfied` bound on `register_grpc_service`. Fail-early-at-startup
makes this less urgent than the controller case was.

## 11. (User-requested, next session) Sync scheduled methods: async bridge for direct-call interception

Requested 2026-07-08, right after the item-5 DecoSlot follow-up. Today a
**sync** `#[scheduled]` method with interceptors runs its chain only around
scheduler ticks — the body can't await the chain, so direct in-code calls
bypass it (async methods already intercept every call path).

Bridge options analyzed (2026-07-08):

- **`block_on` in the sync body — REJECTED.** The body already runs on a
  tokio worker (tick = async task, direct call = async handler);
  `Handle::block_on` panics inside a runtime, `block_in_place` starves the
  worker and panics on `current_thread`. Never in generated code.
- **`tokio::spawn` fire-and-forget — weak.** No deadlock, but the caller no
  longer observes completion and a `Result` return's error is lost (only
  loggable). Tolerable for `()`, wrong for `Result`.
- **Async promotion of the dispatch wrapper — RECOMMENDED.** Generate the
  same hidden-inner + dispatch-wrapper split as async methods, but emit the
  wrapper as `async fn` even though the source is `fn`. Direct callers (all
  in async contexts anyway) get a clear "consider using `.await`" error;
  result propagation and completion semantics preserved; zero runtime risk.
  Cost is DX only: a generated signature differing from the source — must be
  clearly documented (book + guards-interceptors.md), ideally with a macro
  note pointing at the promotion when a caller forgets `.await`. Also update
  the "sync methods keep the task-level chain" carve-outs added by item 5
  (wrapping.rs `generate_scheduled_method`, controller_impl.rs
  `generate_scheduled_tasks` sync arm — the task-level chain becomes
  unnecessary since the task can call the now-async wrapper).
