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
- **Full runs** — weekly (Sunday 02:00 UTC, every crate in `ALL_PACKAGES`),
  manual (`gh workflow run mutants.yml -f packages="r2e-core r2e-http"` or
  `-f packages=all`; only once the workflow is on master), or by adding a
  label to a PR (runs on the PR branch, no merge needed): `mutants-full` for
  every crate, `mutants:<crate>[,<crate>…]` for only those (e.g.
  `mutants:r2e-test,r2e-data-diesel`; labels are capped at 50 characters, so
  use one label per crate for long lists — each label is its own run).
  - `plan` lists each crate's mutants (parse only, no build) and splits it
    into `ceil(n / MUTANTS_PER_SHARD)` shards; every shard of every crate is
    one job of a single matrix. The repo is public: runners are free, GitHub
    runs ~20 jobs at once and queues the rest (a full `all` run ≈ 50 jobs).
  - `full` runs one shard in place with nextest, per-crate rust-cache.
    `r2e-macros` mutants are tested with `r2e-core`'s tests too (its own
    tests barely exercise the codegen).
  - `report` aggregates `report/<crate>/{caught,missed,timeout,unviable}.txt`
    into one artifact, with a per-crate table and missed-per-file summary.
- Downstream coverage: a crate's mutants are tested by its own tests only.
  Add `--test-package <crate>,<consumer>` (as for `r2e-macros`) when a crate's
  behaviour is mostly asserted by its consumers — at the cost of building
  them for every mutant.
