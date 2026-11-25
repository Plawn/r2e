# Workspace — mutation testing, first CI run (2026-11-25)

CI run `45582247997` (PR #172, `mutants-full` label, commit `0835aab3`),
cargo-mutants 27.1.0, `.cargo/mutants.toml` as committed, nextest, each crate
tested by **its own tests only** (`r2e-macros` also by `r2e-core`'s). Before
any fix. Raw lists per crate: `<crate>/missed.txt`, `<crate>/timeout.txt`
(caught/unviable are left out: bulk, no action).

**Partial.** The run was cancelled after ~4 h when the workflow was fixed:

- `r2e-core` shard 7/12 and `r2e-macros` shard 7/10 were cancelled — their
  counts below miss one shard each. `r2e-core`'s complete local run is in
  `../r2e-core-2026-11-25/`.
- `r2e-data-diesel`, `r2e-test`, `r2e-openfga-model`: no result, CI
  setup failures (see "CI failures" below), re-run with `mutants:<crate>`
  labels. `r2e-test` came back from run `45639646766` (commit `e03703df`,
  `cargo test`, not nextest) and is in the table. Diesel and openfga-model
  failed again (second row of fixes below) and are not.

| crate | caught | missed | timeout | unviable |
|---|---|---|---|---|
| r2e-core (11/12 shards) | 979 | 529 | 28 | 747 |
| r2e-macros (9/10 shards) | 216 | 808 | 3 | 756 |
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
| r2e-scheduler | 80 | 6 | 3 | 48 |
| r2e-openapi | 61 | 1 | 0 | 21 |
| r2e-rate-limit | 29 | 0 | 0 | 27 |

## First observations (not triaged)

- **r2e-rt: 0 caught.** Its only test target (`tests/rt`) exercises none of
  the mutated functions (`JoinError::is_panic/is_cancelled`,
  `JobHandle::abort/is_finished`, `set_control_plane`,
  `control_plane_handle`, `yield_now`) — the drain/cancel guards of r2e-core
  rely on them.
- **r2e-macros: 808 missed** even with r2e-core's tests: most codegen
  branches are never compiled by them. Candidate: add `r2e-compile-tests`
  (and the example apps) to its `--test-package`.
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
- Well covered: r2e-openapi, r2e-rate-limit, r2e-scheduler.

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
