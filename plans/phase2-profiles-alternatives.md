# Phase 2 — Profiles & Bean Alternatives (remaining items)

Most of this plan shipped. Implemented and verified in code:

- Active-profile resolution (`with_profile` > `R2E_PROFILE` > `r2e.profile` > `"default"`),
  stored on `BuilderConfig::active_profile` — `r2e-core/src/builder/mod.rs:166`.
- `active_profile()` / `profile_is()` inspectors — `r2e-core/src/builder/nostate.rs:215,224`.
- Profile-conditional wiring via `.when(cond, |b| …)` — `r2e-core/src/builder/mod.rs:311`.
  The `with_bean_for_profile` / `with_*_when` family from the original plan was
  deliberately **dropped** by the DI refactor (§1d of `docs/claude/di-builder-refactor.md`):
  a runtime flag cannot change the compile-time provision list `P`; the compile-time-safe
  path is `#[producer] -> Option<T>`.
- Default/alternative beans: `with_default_bean` / `with_default_async_bean` /
  `with_default_producer` + per-registration `overridable` flag and last-wins resolution —
  `r2e-core/src/builder/nostate.rs:235`, `r2e-core/src/beans.rs:522,1574`.
- Tests: `r2e-core/tests/di/defaults.rs`, `r2e-core/tests/builder/overrides.rs:84`,
  `r2e-core/tests/builder/state_wiring.rs:365`.

---

## Remaining

### 1. (Deferred, optional) Macro sugar `#[bean(profile = "dev")]`

Not implemented — `r2e-macros` has no `profile` attribute parsing.

```rust
#[bean(profile = "dev")]
impl FakeMailer { fn new() -> Self { Self } }
```

Would generate `const PROFILE: Option<&'static str>` on the `Bean` impl and let the
registry filter at registration time. **Open design question:** this conflicts with the
DI-refactor rule that runtime conditions must not remove a type from `P`. If pursued,
it must degrade to an `Option<T>` slot, not a silently missing bean.

### 2. (Deferred) Guaranteed profile groups

Not implemented. `with_profiled_group::<…>() / profile_bean::<B>("dev") / end_group()` —
exactly one impl always present so the type stays in `P`. The plan itself recommended
deferring this; the documented workaround is a wrapper enum / `#[producer]` that switches
on `#[config("r2e.profile")]`.

### 3. Test gaps (small)

- No test asserts `R2E_PROFILE` env-var precedence directly — the two existing tests
  (`config/loader.rs:68`, `builder/state_wiring.rs:379`) *skip* their assertion when the
  variable is set instead of setting it under `support::env_lock()`.
- No test asserts the `"default"` fallback when neither `R2E_PROFILE` nor `r2e.profile`
  is present.
