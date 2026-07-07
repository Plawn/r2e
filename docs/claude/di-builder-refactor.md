# DI & Builder Refactor — DX + Compile-Time Roadmap

Status: **Phases 1 & 2 complete** (landed on `refactor/di-builder-dx-ct`); Phase 3
pending. Tracks a multi-phase refactor of the DI subsystem and `AppBuilder`. Each
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
| 3 | Correctness & cleanup | ⏳ pending |
| 4 | Controllers as graph-resolved beans | 📋 planned — approach **A3 validated by spike**, see `plan-controllers-as-beans.md` |
| 5 | Feature modules (closed subgraphs) | 📋 planned — see `plan-feature-modules.md` |

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

## Phase 3 — Correctness & cleanup

- **Readable `build_state` panic**: `.unwrap_or_else(|e| panic!("...: {e}"))` —
  `Display` exists (beans.rs:525) but `.expect` uses `Debug` and swallows it.
- **Real cycle path**: in `topological_sort` (beans.rs:1286-1292), replace "all
  nodes with `in_degree>0`" with a DFS extracting the actual A→B→C→A path.
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

## Phases 4 & 5 — planned separately (fresh session)

Two forward-looking, higher-ambition changes have their own plan files:

- **Phase 4 — Controllers as graph-resolved beans** (`plan-controllers-as-beans.md`):
  build controller cores from the `BeanContext` by type (`ctx.get`) instead of
  from a hand-written state struct by field name, so a controller is "a bean like
  any other" and the manual `Services` struct disappears. Recommended path (A3):
  the state is the provision list `P` materialized as a type-level HList, giving
  monomorphized indexed access (per-request perf iso) **and** no hand-written
  struct. **A3 was validated by a Fable spike** (monomorphized to single-load
  field access even at depth 63; two constraints folded into the plan: extractor
  impls carry the index witness in trait/`Self` generics, and apps >~127 beans
  need `#![recursion_limit]`). Fallbacks A1/A2 documented but strictly worse.
- **Phase 5 — Feature modules** (`plan-feature-modules.md`): Spring/NestJS-style
  `@Module` bundles (providers + controllers + imports/exports) with **compile-time
  encapsulation**. Depends on Phase 4.

**Ordering recommendation:** do Phase 2 (BuiltApp + `builder.rs` split) and Phase 3
(correctness/clippy) first — small, orthogonal, low-risk. Then Phase 4, then Phase 5.
Phase 4 will **revisit the 1b model**: with context-as-state, the user `BeanState`
struct + `#[derive(BeanState)]` become optional/internal, while `build_state!` and
the `P`/`AllSatisfied` bean-presence tracking remain.
