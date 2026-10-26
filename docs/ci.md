# CI test selection

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
```

The comparison uses committed `HEAD`, not uncommitted working-tree changes.
Planning uses only manifest metadata and Python 3.11+; it does not fetch registry
dependencies. Executing a selected Cargo command resolves the locked graph on
the job's toolchain and fails if that resolution cannot be obtained.
