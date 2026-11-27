# Workspace — mutation testing, first CI run (2026-11-25)

CI run `45582247997` (PR #172, `mutants-full` label, commit `0835aab3`),
cargo-mutants 27.1.0, `.cargo/mutants.toml` as committed, nextest, each crate
tested by **its own tests only** (`r2e-macros`: see its own section). Before
any fix. Raw lists per crate: `<crate>/missed.txt`, `<crate>/timeout.txt`
(caught/unviable are left out: bulk, no action).

**Partial.** The run was cancelled after ~4 h when the workflow was fixed:

- `r2e-core` shard 7/12 was cancelled — its counts below miss one shard. `r2e-core`'s complete local run is in
  `../r2e-core-2026-11-25/`.
- `r2e-data-diesel`, `r2e-test`, `r2e-openfga-model`: no result, CI
  setup failures (see "CI failures" below), re-run with `mutants:<crate>`
  labels. `r2e-test` came back from run `45639646766` (commit `e03703df`,
  `cargo test`, not nextest). Diesel and openfga-model failed again
  (second table of fixes below), then came back from runs `45649915524`
  and `45649844254` (commit `7e2f1c2f`). All three are in the table, so
  every crate now has results.

| crate | caught | missed | timeout | unviable |
|---|---|---|---|---|
| r2e-core (11/12 shards) | 979 | 529 | 28 | 747 |
| r2e-macros (+ core, compile-tests, example-app) | 943 | 0 | 1 | 1023 |
| r2e-mcp | 1042 | 285 | 16 | 420 |
| r2e-test | 123 | 87 | 5 | 164 |
| r2e-http | 49 | 81 | 0 | 92 |
| r2e-openfga | 112 | 73 | 0 | 102 |
| r2e-events | 106 | 60 | 6 | 38 |
| r2e-oidc | 155 | 47 | 0 | 70 |
| r2e-tenant | 169 | 47 | 10 | 117 |
| r2e-tracing | 127 | 47 | 0 | 29 |
| r2e-grpc | 34 | 37 | 0 | 82 |
| r2e-rt | 0 | 36 | 0 | 57 |
| r2e-security | 120 | 27 | 2 | 60 |
| r2e-prometheus | 50 | 25 | 0 | 14 |
| r2e-data-sqlx | 21 | 19 | 0 | 128 |
| r2e-static | 40 | 17 | 0 | 26 |
| r2e-utils | 9 | 10 | 0 | 8 |
| r2e-executor | 19 | 8 | 1 | 30 |
| r2e-cache | 9 | 6 | 0 | 22 |
| r2e-openfga-model | 171 | 20 | 8 | 25 |
| r2e-data-diesel (no `mysql`) | 15 | 11 | 0 | 203 |
| r2e-scheduler | 80 | 6 | 3 | 48 |
| r2e-openapi | 61 | 1 | 0 | 21 |
| r2e-rate-limit | 29 | 0 | 0 | 27 |

## First observations (not triaged)

- **r2e-rt: 0 caught.** Its only test target (`tests/rt`) exercises none of
  the mutated functions (`JoinError::is_panic/is_cancelled`,
  `JobHandle::abort/is_finished`, `set_control_plane`,
  `control_plane_handle`, `yield_now`) — the drain/cancel guards of r2e-core
  rely on them.
- **r2e-macros: 0 missed** once tested with `r2e-compile-tests` and
  `example-app` too (808 with r2e-core's tests alone): see below.
- **r2e-static `parse_byte_range`** (HTTP `Range` bounds, `>`/`>=`,
  `||`/`&&`, length `-`) — off-by-one goes unnoticed.
- **r2e-tracing `push_json_escaped`** — the `< 0x20` control-character guard
  is untested (log injection in JSON logs).
- **r2e-http QUIC** — `serve_h3`, `handle_h3_request` body-size checks,
  `QuicEndpoint::{close, wait_idle}`.
- **r2e-test** — `TestResponse` accessors (`header`, `cookie(s)`,
  `content_type`, `is_json`, `json_optional`), `parse_set_cookie`,
  `json_shape_errors` (shape assertions accept anything), `assert_sse_*`
  `&&`→`||`, `tokenize_path`/`resolve_path`; 18 in `ws.rs`. A test helper
  whose assertion never fails turns into false greens in every consumer.
- **r2e-openfga-model** — `scan_braces` depth counting (brace matching
  is never tested for nesting), `is_identifier` → `true`, `tokenize`
  identifier guard, `parse_param_type` `"bool"` arm, `strip_comment`,
  `last_line_no` (error line numbers).
- **r2e-data-diesel** — `PoolHealth::affects_readiness`, `DbPool::{last_error,
  record_error}`, `ManagedTx::abort` → `()` (rollback on abort unchecked),
  `!` in the plugin `build`s. 203 unviable: generic `Conn` code.
- Well covered: r2e-openapi, r2e-rate-limit, r2e-scheduler.

## r2e-macros with its consumers' tests (2026-11-27)

CI run `45973310310` (commit `f89947c7`), `--test-package
r2e-macros,r2e-core,r2e-compile-tests,example-app --timeout 900`, `cargo
test` (not nextest), 99 shards of 20. 98 shards finished: 938 caught, 0
missed, 0 timeout, 1011 unviable. Shard 69/99 hung (one mutant makes the
macro loop at expansion time, and the build phase had no timeout) and hit
the 350 min job limit; its mutants were re-run locally with `--build-timeout
600`: 18 found (of the shard's 20), 5 caught, 12 unviable, 1 timeout
(`route.rs:391:20` `+=`→`*=` in `extract_doc_comments`: `desc_start` never
advances — a real hang, counts as caught).

**Every viable mutant of r2e-macros is caught.** Nothing to triage. The
first run's 808 missed were codegen branches only `r2e-compile-tests`
(trybuild `.stderr`) and `example-app` compile and run. Half the mutants are
unviable (generated code that no longer type-checks).

Two earlier attempts measured nothing and are not counted:

| run | result | cause | fix |
|---|---|---|---|
| `45657601293` | 21 caught, 921 timeout, cancelled | auto timeout = 120 s floor; trybuild alone takes ~3 min | `--timeout 900`, shards of 20 |
| `45770605284` | 20 caught, 922 timeout, cancelled | nextest: example-app's ordered suites fail (one process per test); `boot_failure` tests waited forever on a boot binary that a mutant had stopped from failing | `cargo test`; `boot_failure.rs` kills the child after 30 s |

Then `--build-timeout 600` for shard 69's hang. The baseline only builds
the mutated package, so none of this showed before the mutants ran.

## CI failures of this run (fixed in the workflow since)

| crate | cause | fix |
|---|---|---|
| r2e-data-diesel | `all_features` enables `mysql` → `mysqlclient-sys` build fails, no libmysqlclient on the runner | `libmysqlclient-dev` in the apt step |
| r2e-test | baseline fails (exit 4): ordered suites wait for earlier cases of the same process, nextest runs one process per test → 60 s watchdog panic (`ordering.rs:282`) | r2e-test mutants run with `cargo test` |
| r2e-openfga-model | mutant `parser.rs:114 next_raw -> Some((0, ""))` loops while allocating, fills the 16 GB runner before the test timeout → runner shutdown, shard results lost | `ulimit -v 10485760` (10 GiB per process) in the mutants step |

Second attempt (runs `45639654093`, `45639661961`), both crates failed again:

| crate | cause | fix |
|---|---|---|
| r2e-data-diesel | `libmysqlclient-dev` installs, but mysqlclient-sys 0.5.2 rejects its version (`24.2.13 is not supported`) | no `mysql` for mutants: `features = sqlite, postgres, tenant` instead of `all_features` |
| r2e-openfga-model | both shards report TIMEOUT on a looping mutant (`parser.rs:104` `+=`→`*=`, `next_raw`, `tokenize` 514), then the runner dies: nextest runs each test in its own process group, cargo-mutants' kill on timeout misses the test, the orphan keeps allocating | run without nextest (one `cargo test` process, killed with its group) |

## Failed attempt: downstream confirmation of r2e-core P1/P2 gaps

Re-testing the 78 P1/P2 missed mutants of `../r2e-core-2026-11-25/` with
`--test-package r2e-core,r2e-security,r2e-openapi,example-app` locally gave
48/48 TIMEOUT before being stopped: the 120 s floor is below the unmutated
run time of four crates' tests. To redo with a higher `--timeout` (or in CI
with a `mutants:` label and `--test-package`). Still to do.
