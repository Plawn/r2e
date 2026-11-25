# Mutation testing (cargo-mutants)

cargo-mutants rewrites one piece of code at a time (a function body replaced
by `Default::default()`, a `<` turned into `<=`, a match arm deleted) and runs
the crate's tests. A **missed** mutant is a behaviour change no test notices.

## Configuration

- `.cargo/mutants.toml` — applies to every run, local or CI:
  - `all_features = true`: feature-gated code (`ws`, `multipart`, `openapi`,
    `dev-reload`, `quic`) is otherwise never compiled under test, and its
    mutants all show up as missed.
  - `--lib --tests`: no doctests (almost all `ignore`). A mutant only a
    doctest kills shows as missed.
  - `minimum_test_timeout = 120`: the auto timeout is too tight once several
    copies build in parallel (first r2e-core run: 484 false timeouts at 28 s).
  - `profile = "mutants"` (root `Cargo.toml`): `test` without debuginfo.
  - `exclude_re`: mutants no test can kill — `Debug`/`Display` impls, impls on
    `()`/`TNil`, forwarding `poll_ready`, log-only helpers. Extend it rather
    than writing tests that assert formatting.
- Known unkillable mutants still generated (ignore them in reports): `<` →
  `<=` on `Duration` elapsed comparisons, default trait methods returning
  `vec![]`.

## Outcomes

| Outcome | Meaning |
|---|---|
| caught | a test failed — good |
| missed | tests pass with the mutation — a test gap (or an equivalent mutant) |
| timeout | tests hung — counts as caught, *if* the timeout is generous enough |
| unviable | the mutant does not compile (`Pin<Box<dyn Future>>` → `Default`, …) — ignore |

## Running locally

```bash
cargo install cargo-mutants --locked
RUSTC_WRAPPER="" cargo mutants -p r2e-core -j 3 --cargo-arg=--target-dir=target
RUSTC_WRAPPER="" cargo mutants -p r2e-core --iterate -j 3 --cargo-arg=--target-dir=target  # re-test missed/timeout only
RUSTC_WRAPPER="" cargo mutants --in-diff <(git diff master...) --in-place              # only your changes
```

- `-j N` copies the tree N times; the workspace's global target dir would make
  the copies fight over one lock, so `--cargo-arg=--target-dir=target`
  (relative — resolved inside each copy). Each copy rebuilds the deps once.
- `RUSTC_WRAPPER=""` only inside the Claude sandbox (sccache is blocked there).
- `--test-tool=nextest` (needs `cargo install cargo-nextest`) stops at the
  first failing test, which is how most mutants die.
- A full r2e-core run is ~2300 mutants and takes hours; prefer `--in-diff`,
  `-f <file>` or `--re <regex>` while working on a gap.

## CI (`.github/workflows/mutants.yml`)

- **Pull requests** — `diff` job: `--in-diff` on the PR's `.rs` changes,
  in place, nextest. Fails when a mutant on a changed line is missed; the
  missed list is in the job summary. Informational: not a required check.
- **Weekly (Sunday 02:00 UTC) + manual** — `full` job: 8 shards
  (`--shard k/8`) of `r2e-core` (manual input: any package, or `workspace`),
  then `report` aggregates `caught/missed/timeout/unviable.txt` into one
  artifact and a per-file summary.
