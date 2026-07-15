# r2e-compile-tests

Compile-time tests for R2E macros — verifies error messages using `trybuild`.

## Overview

Contains `compile-fail` and `compile-pass` test fixtures that ensure R2E's proc macros (`#[controller]`, `#[routes]`, `#[bean]`, etc.) produce the expected compile errors for invalid usage and compile successfully for valid usage.

## Structure

```
r2e-compile-tests/
  compile-fail/   # .rs files expected to fail with specific error messages
  compile-pass/   # .rs files expected to compile successfully
  tests/          # trybuild test runner
```

## Running

```bash
cargo test -p r2e-compile-tests
```

## License

Apache-2.0
