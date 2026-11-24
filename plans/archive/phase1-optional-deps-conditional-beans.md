# Phase 1 — Optional Dependencies + Conditional Bean Registration

> **Archived 2026-11-24.** Status: shipped (`Option<T>` beans, `.when` / `config_flag` / `profile_is`); the one remaining item (example-app demo of a config-driven `#[producer] -> Option<T>`) is tracked in `docs/claude/roadmap.md` § Open items.

**Status: essentially complete / superseded — only one item remains.**

Context (for the remaining item only):

- `Option<T>` is now a **first-class bean type**: a `#[producer] -> Option<T>`
  always registers the `Option<T>` slot, and consumers declare `Option<T>` as a
  hard dep. Shipped and tested (`r2e-core/tests/di/{optional,option_type,option_config}.rs`).
- The conditional-builder zoo (`with_bean_when` / `with_bean_on_config` / …) was
  **deliberately rejected** in DI refactor phase 1d (see
  `docs/claude/di-builder-refactor.md`). The blessed replacements —
  `.when(cond, f)`, `config_flag(key)`, `profile_is(profile)` — are shipped
  (`r2e-core/src/builder/mod.rs`, `r2e-core/src/builder/nostate.rs`). Do not
  re-propose `_when`-style registration methods.

## Remaining

- [ ] **Example coverage** — no example app demonstrates the blessed conditional
      pattern. Add to `examples/example-app` a `#[producer] -> Option<T>` whose
      `Some`/`None` is driven by a config flag (e.g. `builder.config_flag(...)`
      or a `#[config("…enabled")] bool` producer param), plus a consumer bean or
      controller that injects `Option<T>` and degrades gracefully when `None`.
