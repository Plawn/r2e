# CI

Seven workflows live in `.github/workflows/`. All of them run on Linux runners;
none runs clippy or rustfmt (run those locally, see `CONTRIBUTING.md`).

| Workflow | File | Triggers | What it does |
|---|---|---|---|
| Tests | `test.yml` | push to `master`, PRs, manual | Script self-tests, version invariants, affected-crate tests, feature legs, trybuild, MSRV |
| Dependency boundaries | `boundaries.yml` | push to `master`, PRs, manual | axum / tokio / JSON-codec containment, OpenFGA proto drift |
| LLM docs | `llm-docs.yml` | push to `master`, PRs, manual | `llm/` + `llm-full.txt` consistency |
| Performance | `perf.yml` | push to `master` and PRs touching the hot path, manual | Instruction-count gate |
| Release | `release.yml` | after Tests succeeds on a `master` push | Publishes changed crates, tags, GitHub releases, registry smoke |
| Registry smoke | `registry-smoke.yml` | manual only | Builds a throwaway app against crates.io |
| Deploy mdBook to GitHub Pages | `deploy-docs.yml` | push to `master` under `docs/book/**`, manual | Builds and publishes the book |

## Tests (`test.yml`)

Three jobs:

- **`changes`** plans the run. It first runs the script self-tests
  (`python3 -m unittest discover -s scripts/tests -p 'test_*.py'`, i.e.
  `test_ci_affected.py` and `test_versions.py`), then `scripts/check-versions.sh`
  (per-crate version invariants, pins, changelogs, the "nothing left behind"
  rule while a release is in flight), then `scripts/ci_affected.py plan`.
- **`test`** runs `ci_affected.py run test` on the selected packages, plus the
  legs that need non-default features:
  - `dev-reload` (when `r2e-core` is affected):
    `cargo check -p r2e-core --features dev-reload --all-targets`,
    `cargo test -p r2e-core --features dev-reload --test runtime` and
    `--test dev_reload`;
  - MCP loopback: `cargo test -p r2e-mcp --features loopback,testing --test server`
    and `cargo check -p r2e --features mcp-loopback`;
  - scheduler test clock: `cargo test -p r2e-scheduler --features testing --test testing`;
  - trybuild: `ci_affected.py run compile-tests` (when `r2e-compile-tests` is affected).
  The allocation budgets (`r2e-core/tests/perf_budget`) are ordinary tests and
  run here.
- **`msrv`** reads `rust-version` from `[workspace.package]`, installs that
  toolchain and runs `ci_affected.py run msrv` (`cargo check` of the selected
  crates, or the whole workspace).

Package selection is described in [CI test selection](#ci-test-selection) below.

## Dependency boundaries (`boundaries.yml`)

- `scripts/check-dep-boundary.sh`: only `r2e-http` depends on axum, only
  `r2e-rt` (and the exempt crates) on tokio.
- `scripts/check-source-boundary.sh`: source-level checks (raw axum / tokio /
  `serde_json` typed calls, `#[tokio::test]`, …) against shrink-only baselines.
- `scripts/generate-openfga-proto.sh --check`: regenerates the committed
  OpenFGA gRPC client (installs `protoc`) and fails on drift.

Unconditional: these run on every PR.

## LLM docs (`llm-docs.yml`)

`scripts/check-llm-docs.sh`: a pure text check that the `tokens:` front matter
of `llm/*.md` is current and that `llm-full.txt` matches the hub and spokes.
Fix locally with `scripts/check-llm-docs.sh --update`. The snippets themselves
compile in the Tests workflow (`llm-doctests`, selected when `llm/` or `llm.txt`
change).

## Performance (`perf.yml`)

Instruction-count gate for the request hot path and the bean graph
(`r2e-core/benches/instructions.rs`, gungraun + Valgrind), driven by
`scripts/perf-instructions.sh [base-sha]`, which compares the PR against its
base. It runs when `r2e-core`, `r2e-http`, `r2e-macros`, `r2e-rt`,
`r2e-tracing`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, the script or
the workflow change. Adding the `perf-regression-accepted` label to a PR keeps
the numbers printed but lets the job pass; the workflow re-runs on label
changes.

## Release (`release.yml`)

Runs after the Tests workflow succeeds on a push to `master` (never on PRs).
A release is a change of one or more crate versions made by
`scripts/bump-version.sh` in a `release: …` PR; every other push is a no-op.

1. `scripts/check-versions.sh`.
2. **Decide** (`scripts/versions.py decide`): the crates whose version changed
   against the previous master and whose `<crate>-vX.Y.Z` tag does not exist
   yet. An untagged but unchanged version (an unfinished earlier release on an
   older commit) fails the step.
3. `scripts/publish-crates.sh --yes --resume` with crates.io trusted publishing
   (OIDC); crates already on crates.io at their version are skipped.
4. Tags each crate `<crate>-vX.Y.Z` on that exact commit and creates its GitHub
   release from its `CHANGELOG.md` section. Tags come last: a tag means the
   crate's release is complete.
5. `scripts/smoke-registry.sh`: builds a throwaway app against crates.io alone.

A failed publish is resumed with **Re-run failed jobs** on the same run. Never
release from a later commit; the run refuses to. `r2e-cli` is held back from
crates.io by design.

## Registry smoke (`registry-smoke.yml`)

Manual (`workflow_dispatch`, optional `version` input). Runs
`scripts/smoke-registry.sh [version]` on stable: the same check as the last
release step, for a release whose smoke step failed for an unrelated reason
(re-running Release would skip it, every crate being tagged) or to check an
older version. It neither publishes nor tags. Locally,
`scripts/smoke-registry.sh --patch` builds the same app against the working
tree before a release.

## Deploy mdBook (`deploy-docs.yml`)

On pushes to `master` that touch `docs/book/**` (or manually): `mdbook build
docs/book`, then deploys to GitHub Pages.

## CI test selection

The Tests workflow runs the full workspace on pushes to `master` and manual
dispatches. Pull requests use `scripts/ci_affected.py` to select changed crates
and their transitive consumers for both tests and the MSRV check. The selected
packages and fallback reason appear in the workflow summary.

The selector compares the PR base SHA with the tested merge checkout, including
both paths of renamed files. `cargo metadata --no-deps` supplies all declared
normal, build, dev, optional and target-specific dependency edges. Nested crates
are owned by their deepest workspace directory.

For dependency updates:

- A member manifest change selects that member and its consumers.
- Changes confined to `[workspace.dependencies]` select consumers of the changed
  entries, including dependency aliases.
- A `Cargo.lock` diff selects consumers of changed package records, walking the
  union of the old and new lockfile graphs. Multiple versions with the same name
  are intentionally grouped conservatively.

Changes to workspace settings, toolchains, Cargo configuration, CI/scripts,
deleted manifests, unknown paths, or unsuccessful analysis run the full suite.
Only explicitly listed prose paths can skip Rust jobs. `llm/` and `llm.txt`
select the CLI and LLM doctests; `docs/features/` selects the CLI because these
documents are embedded in its binary. Changes inside a crate, including its
fixtures and documentation, select that crate.

Selected test/check commands explicitly enable each selected package's features
from the full workspace resolution (`cargo metadata --locked`). This preserves
test gates normally enabled by other workspace members. Dependencies can still
have a different feature union in a partial build; the full runs on `master`
and manual dispatch retain the workspace-wide integration check.

The `dev-reload` legs run when `r2e-core` is affected, including through its
optional dependencies. Trybuild runs separately when `r2e-compile-tests` is
affected. An empty package selection never invokes Cargo with an implicit
default package set.

Version/release checks and selector regression tests run on every invocation.
The separate boundary, OpenFGA regeneration and LLM consistency workflows remain
unconditional. The existing Rust job names remain stable for required checks;
an upstream planning or version-check failure explicitly fails those jobs.
Master continues to populate the shared Rust caches.

Local inspection (read-only):

```bash
python3 scripts/ci_affected.py plan --base <base-commit>
python3 scripts/ci_affected.py plan  # full plan
python3 -m unittest discover -s scripts/tests -p 'test_ci_affected.py'
python3 -m unittest discover -s scripts/tests -p 'test_versions.py'
```

The comparison uses committed `HEAD`, not uncommitted working-tree changes.
Planning uses only manifest metadata and Python 3.11+; it does not fetch registry
dependencies. Executing a selected Cargo command resolves the locked graph on
the job's toolchain and fails if that resolution cannot be obtained.
