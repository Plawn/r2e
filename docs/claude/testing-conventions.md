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

## Async test attribute

**Async tests use `#[r2e_core::test]`** — `#[r2e::test]` in crates that depend on the
facade (examples, `r2e-edition2024-tests`, `r2e-featureless-tests`). Never `#[tokio::test]`.

The R2E attribute is the test-side twin of `#[r2e::main]`: a multi-thread Tokio runtime,
`init_tracing_from_config()` called before the body, and the app-test extensions
(`app = MyApp` boots a `TestApp`, `#[inject]` parameters pull beans, `with = |b| …` pins
mocks, `jwt = false` skips `TestJwt`). A test written with `#[tokio::test]` opts out of all
of that and reads as foreign code in a workspace whose own rule is "the runtime is reached
through the facade" (`r2e_core::rt`, `scripts/check-source-boundary.sh`).

| `#[tokio::test…]` | `#[r2e_core::test…]` |
|---|---|
| `#[tokio::test]` | `#[r2e_core::test]` (already multi-thread) |
| `#[tokio::test(flavor = "multi_thread")]` | `#[r2e_core::test]` |
| `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` | `#[r2e_core::test(worker_threads = 2)]` |
| `#[tokio::test(flavor = "current_thread")]` | `#[r2e_core::test(flavor = "current_thread")]` |
| `#[tokio::test(start_paused = true)]` | `#[r2e_core::test(flavor = "current_thread", start_paused = true)]` |
| a test that installs its own subscriber or asserts on log output | add `tracing = false` |

Exempt, because they sit below `r2e-core` in the dependency graph and cannot name the
macro: `r2e-rt`, `r2e-http`, `r2e-macros`. Also exempt: `r2e-devservices` (dev-only
testcontainers harness, deliberately without an `r2e-core` dependency). Everything else —
including `r2e-core`'s own tests, `r2e-test` and the data/events backends — uses the R2E
attribute.

**Enforced by CI.** `scripts/check-source-boundary.sh` has a `tokio-test` group that counts
`#[tokio::test` per git-tracked file under any `tests/` directory against
`scripts/boundaries/tests-baseline-tokio-test.txt`. The baseline is the pre-rule debt and
**only ever shrinks**: a new occurrence, or a file that was clean gaining one, fails the
check; when a PR converts tests, run `scripts/check-source-boundary.sh --update` in the same
PR so the baseline follows. Converting the backlog is welcome crate by crate — check each
test still passes with tracing initialised, and add `tracing = false` where it does not.
