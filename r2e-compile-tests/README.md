# r2e-compile-tests

Compile-time tests for R2E macros — verifies error messages using `trybuild`.

## Overview

Contains `compile-fail` and `compile-pass` test fixtures that ensure R2E's proc macros (`#[controller]`, `#[routes]`, `#[bean]`, etc.) produce the expected compile errors for invalid usage and compile successfully for valid usage.

## Structure

Fixtures are grouped by subsystem (mirroring the routing table in the
workspace `CLAUDE.md`), and each subsystem splits into `fail/` (expected to
fail with a specific error) and `pass/` (expected to compile). Every
`fail/*.rs` has its `.stderr` sibling next to it.

```
r2e-compile-tests/
  cases/
    <subsystem>/
      fail/   # .rs + matching .stderr expected to fail
      pass/   # .rs expected to compile successfully
  tests/      # trybuild test runner (globs cases/*/fail/*.rs, cases/*/pass/*.rs)
```

Subsystems: `auth`, `beans`, `config`, `controller`, `decorators`, `events`,
`executor`, `grpc`, `http`, `modules`, `openfga`, `plugins`, `routing`,
`scheduler`, `testing`. Add a new fixture under the matching subsystem's
`fail/` or `pass/` directory — the glob picks it up automatically.

## Running

```bash
cargo test -p r2e-compile-tests
```

## License

Apache-2.0
