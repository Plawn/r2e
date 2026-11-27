# Mutation testing — proposed test work (2026-11-25)

Built from `r2e-core-2026-11-25/` and `workspace-2026-11-25/`. Nothing is
written yet: pick lots, then each lands as tests in the crate's existing
target (`<crate>/tests/<target>/`), `#[r2e_core::test]` for async, and is
checked by re-running cargo-mutants on the touched file
(`cargo mutants -p <crate> -f <file>`): the listed mutants must turn
caught.

Size: S < ½ day, M ≈ 1 day, L > 1 day. "Kills" = missed mutants the lot
should turn caught.

## Lot 1 — security and access control (do first)

| # | where | what to test | kills | size |
|---|---|---|---|---|
| 1.1 | r2e-mcp `session/access.rs` → `tests/server` | `McpView::{has_role, has_scope}` deny when the role/scope is absent (mutants → `true` survive), `Access::{own_gated, global_gated}` (→ `true`), `McpViewFilter::listed`, `subject`/`principal`/`identity`. A tool/resource/prompt hidden or refused for a principal lacking the role or scope, listed for one who has it | 22 | M |
| 1.2 | r2e-security `jwks.rs` → `tests/jwks.rs` | `JwksCache::try_refresh` (→ `Ok(())`, every `!`/`&&` 415–430: refresh cooldown and "had cached key" paths), `get_shared_key` guard 357, `read_body_limited` bound at exactly the limit / limit+1 (`>` vs `>=`) | 15 | M |
| 1.3 | r2e-core `decorators/claims.rs`, `guards.rs` (A.1) | First confirm downstream: `mutants:` run of r2e-core with `--test-package r2e-core,r2e-security,r2e-mcp`. Then test only what stays missed: `Audience::contains` (→ `true`!), realm/client roles, scopes, `forwarded_for`/`forwarded_ip`, `PreAuthGuardContext::{path, path_param}` | ≤ 32 | S + M |
| 1.4 | r2e-tracing `flat_json.rs`/`push_json_escaped` → `tests/format` | control chars `< 0x20` escaped as `\u00XX` (log injection in JSON logs) | 5 | S |

## Lot 2 — test helpers that cannot fail (false greens everywhere)

| # | where | what to test | kills | size |
|---|---|---|---|---|
| 2.1 | r2e-test `app.rs` → `tests/assertions.rs` | each assertion **fails** on wrong input (`#[should_panic]`): `assert_sse_event`/`assert_sse_data` (`&&`→`||`), JSON shape (`json_shape_errors`: object/array arms, `!=`), `json_type_name` in messages | ~15 | S |
| 2.2 | r2e-test `app.rs` → `tests/cookie.rs`, `tests/app.rs` | `TestResponse::{header, cookie, cookies, content_type, is_json, json_optional}`, `parse_set_cookie` (`name=value; attrs`, no `=`, empty), `tokenize_path`/`resolve_path` on arrays | ~25 | S |
| 2.3 | r2e-test `ws.rs`, `session.rs` | WsTestClient helpers (18), session cookie carry-over (7) | ~25 | M |

## Lot 3 — HTTP edge behaviour

| # | where | what to test | kills | size |
|---|---|---|---|---|
| 3.1 | r2e-static `parse_byte_range` → `tests/embedded.rs` | table of `Range` headers: `bytes=0-0`, `bytes=0-<len-1>`, `bytes=<len>-` (416), `bytes=-N` suffix, `N-M` with `M ≥ len` (clamped), `M < N` | 9 | S |
| 3.2 | r2e-core `error/rejection.rs`, `error/mod.rs` (A.2) → `tests/http` | table rejection → status: body too large 413, wrong content type 415, malformed 400, per extractor (Form, String, RawForm, Bytes, Multipart); `HttpError::from_status` arms | 26 | M |
| 3.3 | r2e-core `web/multipart.rs` (A.3) → `tests/http` | limits at exactly N and N+1 (field size, total size, file count), `take_*` accessors | 20 | M |
| 3.4 | r2e-http `json.rs` → new module in `tests/routing.rs` or `tests/json` | `is_json_content_type` (`application/json`, `+json` suffix, `application/jsonx` rejected, params), `JsonError::{is_data, is_syntax, is_eof, is_io}` | ~20 | S |

## Lot 4 — parsers and config

| # | where | what to test | kills | size |
|---|---|---|---|---|
| 4.1 | r2e-openfga-model → `tests/parser.rs` + `tests/corpus` | `scan_braces` nesting (`{ { } }`, unbalanced → error), `is_identifier` (leading digit, `-`, `_`), `bool` param type, `#` comment stripping at end of line, error **line numbers** (`last_line_no`) | 20 | S |
| 4.2 | r2e-core `config/value.rs`, `de.rs`, `loader.rs` (A.7) → `tests/config` | duration units (`"5m"` = 300 s: `×60` mutant), bool spellings `true/1/yes`, `f64` from int and string, `PathBuf`, seq/enum deserialisation | 28 | M |
| 4.3 | r2e-core `builtins/health.rs` (A.9) → `tests/plugin` or `tests/http` | worst-status aggregation (Down beats Degraded beats Up — the `<` at 316/371), readiness excludes `affects_readiness = false` indicators, cache TTL | 20 | S |

## Lot 5 — runtime primitives and data

| # | where | what to test | kills | size |
|---|---|---|---|---|
| 5.1 | r2e-rt → `tests/rt` | `JoinError::{is_panic, is_cancelled}` (a panicking / aborted task), `JobHandle::{abort, is_finished}`, `CancelToken`/`ShutdownToken::{is_cancelled, child_token, cancelled}`, `in_runtime`, `RuntimeHandle::is_multi_thread`. r2e-core's drain guards rely on these | 36 | S |
| 5.2 | r2e-data-diesel → `tests/tx`, `tests/pool`, `tests/datasource` (sqlite) | `ManagedTx::abort` rolls back (row absent after an erroring handler), `DbPool::{record_error, last_error}`, `PoolHealth::affects_readiness` and `name` | 11 | S |

## Lot 6 — large areas (scope before starting)

- r2e-core shutdown/drain (A.4, 69), bean graph (A.6, 36), WS/SSE (A.10,
  40), sharded runtime (A.12) — need fixtures that boot and stop apps;
  some cases are Linux-only.
- r2e-mcp catalog/handler/uri_template (~90 outside lot 1).
- r2e-http QUIC (12), routing (16).
- r2e-macros — nothing: with `r2e-compile-tests` and `example-app` every
  viable mutant is caught (2026-11-27, `workspace-2026-11-25/REPORT.md`).

## Not proposed

Equivalent mutants (Duration `<`→`<=`, default trait methods returning
`vec![]`, pure accessors): extend `exclude_re` instead. The timeouts are
real hangs and count as caught.

## Suggested order

Lot 1 → 2 → 3.1/3.4/4.1/4.3/5.1/5.2 (all S, ~150 mutants for a few days)
→ 3.2/3.3/4.2 → Lot 6 by area.
