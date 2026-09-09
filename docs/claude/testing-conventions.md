# Testing Conventions — Layout & Rules

Extracted from `CLAUDE.md` (the hub keeps the golden rules). This file is the authoritative
detail on test layout, target grouping, and the reference structure.

**Tests live in `<crate>/tests/` directories, not inline.** Do NOT use `#[cfg(test)] mod tests { ... }` blocks inside source files.

- One test module per source module: `src/foo.rs` → `tests/<subsystem>/foo.rs`
- Use external imports (`use <crate_name>::...`) instead of `use super::*`
- Keep test-only helpers in the test file, not in the source
- If a test needs access to an internal item, add a `pub` accessor or `pub` + `#[doc(hidden)]` — do NOT use `#[cfg(test)] pub(crate)` visibility hacks

**Group test modules into one target per subsystem.** Cargo turns each
`tests/<name>/main.rs` into a target named `<name>`; the sibling files are
plain `mod`s. One binary per subsystem instead of one per file keeps link time
down and gives shared fixtures a home. Do NOT add new top-level `tests/*.rs`
files to a crate that already uses this layout — add a `mod` to the matching
target.

`r2e-core/tests/` is the reference layout:

```
support/mod.rs   # helpers shared across targets (not a target: no main.rs).
                 # Each main.rs pulls it in with
                 #   #[path = "../support/mod.rs"] mod support;
config/          # R2eConfig, ConfigProperties, sections, env overlay,
                 #   secrets, loading, startup validation
di/              # bean graph, async beans, producers, optional/lazy beans,
                 #   defaults, pinned overrides, lifecycle, modules
builder/         # AppBuilder: HList state, overrides, prepared, App trait
controller/      # request façade, core-only path, #[anonymous], injection
                 #   scopes, proxy/catch-all routing
decorators/      # DecoratorSpec, guards, interceptors
plugin/          # Provided/Deps/Late, deferred surface, config, lifecycle
http/            # extractors, errors, SSE/WS, managed resources, HTTP plugins
runtime/         # rt, sharded serving, socket options, tracing
dev_reload/      # hot-patch cycles, live config, rollback — its OWN target:
                 #   `mark_hot_reload_loop()` is process-global and one-way,
                 #   and would make every serving test in a shared binary skip
                 #   its startup lifecycle
```

Conventions inside a target:

- Fixtures used by more than one module go in `<target>/fixtures.rs`; keep
  single-use fixtures next to the test that needs them.
- Feature gates go on the `mod` declaration in `main.rs`
  (`#[cfg(feature = "ws")] mod ws;`), not as `#![cfg(...)]` inside the file.
- Tests that mutate `std::env` share a process now — take
  `crate::support::env_lock()` for the whole test, even when variable names
  don't overlap.

```bash
cargo test --workspace                    # all tests
cargo test -p r2e-core                    # single crate
cargo test -p r2e-core --test config      # one subsystem target
cargo test -p r2e-core --test config sections::   # one module within it
```
