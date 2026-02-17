# R2E Workspace — Test Development Plan (Master)

## Executive Summary

The R2E workspace has **~132 tests** across 16 crates, with coverage concentrated in a few areas (beans/config in r2e-core, role extractors in r2e-security, compile tests in r2e-macros). Most crates have 0% runtime test coverage. This plan proposes **~527 new tests** across all crates, organized into 4 implementation waves.

---

## Current State

| Crate | Existing Tests | Coverage | Plan Tests | Plan Location |
|-------|---------------|----------|------------|---------------|
| r2e-core | 27 | ~5% | 76 | `r2e-core/TEST_DEVELOPMENT_PLAN.md` |
| r2e-macros | 65 | ~30% runtime | 66 | `r2e-macros/TEST_DEVELOPMENT_PLAN.md` |
| r2e-security | 25 | ~30% | 79 | `r2e-security/TEST_DEVELOPMENT_PLAN.md` |
| r2e-events | 6 | ~55% | 28 | `r2e-events/TEST_DEVELOPMENT_PLAN.md` |
| r2e-scheduler | 0 runtime | 0% | 32 | `r2e-scheduler/TEST_DEVELOPMENT_PLAN.md` |
| r2e-data | 0 | 0% | 36 | `r2e-data/TEST_DEVELOPMENT_PLAN.md` |
| r2e-data-sqlx | 0 | 0% | 29 | `r2e-data-sqlx/TEST_DEVELOPMENT_PLAN.md` |
| r2e-cache | 7 | ~44% | 29 | `r2e-cache/TEST_DEVELOPMENT_PLAN.md` |
| r2e-rate-limit | 4 | ~30% | 36 | `r2e-rate-limit/TEST_DEVELOPMENT_PLAN.md` |
| r2e-openapi | 0 | 0% | 40 | `r2e-openapi/TEST_DEVELOPMENT_PLAN.md` |
| r2e-utils | 8 | ~60% | 24 | `r2e-utils/TEST_DEVELOPMENT_PLAN.md` |
| r2e-test | 0 | 0% | 33 | `r2e-test/TEST_DEVELOPMENT_PLAN.md` |
| r2e-cli | ~1 | ~5% | 63 | `r2e-cli/TEST_DEVELOPMENT_PLAN.md` |
| r2e-openfga | 14 | ~70% | 31 | `r2e-openfga/TEST_DEVELOPMENT_PLAN.md` |
| example-app | 26 | ~90%* | 5 (extend) | included in r2e-macros plan |
| **Total** | **~132** | | **~527** | |

*\* 90% of what example-app currently exercises; many features are not tested there at all (WS, SSE, consumers, scheduled, managed, etc.)*

---

## Implementation Waves

### Wave 1: Quick Wins — Pure Logic Unit Tests (~2 days)

No external dependencies, no async, no I/O. Highest ROI.

| Crate | What | Tests | Effort |
|-------|------|-------|--------|
| r2e-core | AppError response mapping | 11 | 1h |
| r2e-core | GuardContext, PathParams, NoIdentity | 14 | 2h |
| r2e-data | Pageable offset, Page total_pages, DataError | 25 | 3h |
| r2e-security | SecurityError, bearer token extraction, AuthenticatedUser helpers | 34 | 4h |
| r2e-rate-limit | RateLimit builder, key generation, InMemoryRateLimiter | 15 | 2h |
| r2e-cache | evict_expired, TTL verification | 8 | 1.5h |
| r2e-cli | Template helpers, field parsing | 24 | 2.5h |
| r2e-openapi | SchemaRegistry, OpenApiConfig | 11 | 2h |
| r2e-utils | Counted, MetricTimed, edge cases | 12 | 2h |
| r2e-openfga | Config, cache edge cases, registry no-cache | 14 | 2.5h |
| **Subtotal** | | **168** | **~22.5h** |

### Wave 2: Async & Integration Tests (~3 days)

Requires tokio runtime, in-memory databases, or HTTP test clients.

| Crate | What | Tests | Effort |
|-------|------|-------|--------|
| r2e-core | Plugin system, built-in plugins (Health, Cors) | 11 | 3h |
| r2e-core | Health checks, interceptor trait | 12 | 3h |
| r2e-data-sqlx | Tx lifecycle, error mapping, CRUD with SQLite | 29 | 8.5h |
| r2e-events | Panic isolation, subscription safety, async handler behavior | 20 | 5h |
| r2e-scheduler | ScheduleConfig, SchedulerHandle, interval execution | 17 | 5h |
| r2e-security | JWT validation with static keys, JwtValidator | 12 | 3h |
| r2e-test | TestJwt self-tests, TestApp/TestResponse assertions | 33 | 7h |
| **Subtotal** | | **134** | **~34.5h** |

### Wave 3: Full Integration & Runtime Behavior (~3 days)

End-to-end tests, cross-crate integration, timing-sensitive tests.

| Crate | What | Tests | Effort |
|-------|------|-------|--------|
| r2e-core | AppBuilder full lifecycle | 17 | 4h |
| r2e-macros | WebSocket, SSE, managed resources, consumers, scheduled (runtime) | 30 | 10h |
| r2e-macros | Guards, middleware, optional identity, config types | 22 | 6h |
| r2e-scheduler | Cron, cancellation, state isolation, plugin lifecycle | 15 | 5.5h |
| r2e-rate-limit | Guard HTTP responses, integration tests | 8 | 3.5h |
| r2e-openapi | Spec generation, plugin routes, docs UI | 19 | 6h |
| r2e-openfga | Guard integration (full check flow) | 6 | 3h |
| **Subtotal** | | **117** | **~38h** |

### Wave 4: Infrastructure, Robustness & Advanced (~2 days)

Concurrency tests, mocking infrastructure, stress tests, CLI integration.

| Crate | What | Tests | Effort |
|-------|------|-------|--------|
| r2e-core | RequestId, SecureHeaders, WebSocket, SSE | 11 | 4h |
| r2e-security | JwksCache with HTTP mocking (wiremock) | 11 | 4h |
| r2e-cache | Concurrent access, singleton isolation fix | 9 | 2.5h |
| r2e-events | Stress tests, consumer integration | 8 | 3h |
| r2e-cli | Code generation, doctor, routes, scaffolding, add | 39 | 10h |
| r2e-openfga | GrpcBackend (requires OpenFGA or mock) | 6 | 4h |
| r2e-macros | Compile tests for untested derives | 12 | 2h |
| r2e-rate-limit | Edge cases, concurrent rate limiting | 4 | 1h |
| **Subtotal** | | **100** | **~30.5h** |

---

## Cross-Cutting Concerns

### Test Infrastructure Needed

| Item | Used By | Priority |
|------|---------|----------|
| `tempdir` dev-dependency | r2e-cli tests | Wave 1 |
| `tokio = { features = ["test-util"] }` | r2e-scheduler, r2e-events | Wave 2 |
| `wiremock` or `mockito` | r2e-security JWKS tests | Wave 4 |
| `tokio-tungstenite` | r2e-macros WebSocket tests | Wave 3 |
| `tracing-test` or `tracing-subscriber` test setup | r2e-utils log verification | Wave 1 |

### Known Issues to Address

1. **Global singleton pollution** (r2e-cache `CACHE_BACKEND`) — fix in Wave 1 or Wave 2
2. **Timing-sensitive tests** — use `tokio::time::pause()` where possible, generous margins otherwise
3. **Test isolation** — each test should create its own fixtures; avoid shared mutable state
4. **Feature flag testing** — r2e-data-sqlx sqlite/postgres/mysql features should each have CI matrix entries

### CI Recommendations

- Run `cargo test --workspace` as the primary test command
- Add `--test-threads=1` for timing-sensitive tests or use `#[serial_test::serial]`
- Consider separate CI jobs for tests requiring external services (OpenFGA, Postgres)
- Add `cargo clippy --workspace -- -D warnings` to catch issues early

---

## Priority Order (If Time-Constrained)

If only implementing a subset, prioritize in this order:

1. **r2e-data Pageable/Page** — pure math, catches real bugs, 30 min
2. **r2e-core AppError** — verifies HTTP status codes, 1h
3. **r2e-security bearer extraction + AuthenticatedUser** — pure logic, 2h
4. **r2e-data-sqlx Tx lifecycle** — critical path, 3h
5. **r2e-scheduler interval execution** — verify tasks actually run, 3h
6. **r2e-events panic isolation** — verify error containment, 1.5h
7. **r2e-core AppBuilder lifecycle** — main framework entry point, 4h
8. **r2e-openapi spec generation** — validate output correctness, 4h
9. **r2e-test TestJwt self-tests** — test infrastructure reliability, 2h
10. **r2e-cli template helpers** — code generation correctness, 1.5h

---

## Per-Crate Plan Locations

Each crate has its own detailed `TEST_DEVELOPMENT_PLAN.md` with:
- Phase-by-phase test tables with exact test names and descriptions
- Effort estimates per phase
- Required dependencies
- Implementation notes

```
r2e-core/TEST_DEVELOPMENT_PLAN.md         — 76 tests, 9 phases
r2e-macros/TEST_DEVELOPMENT_PLAN.md        — 66 tests, 8 phases
r2e-security/TEST_DEVELOPMENT_PLAN.md      — 79 tests, 8 phases
r2e-events/TEST_DEVELOPMENT_PLAN.md        — 28 tests, 6 phases
r2e-scheduler/TEST_DEVELOPMENT_PLAN.md     — 32 tests, 7 phases
r2e-data/TEST_DEVELOPMENT_PLAN.md          — 36 tests, 3 phases
r2e-data-sqlx/TEST_DEVELOPMENT_PLAN.md     — 29 tests, 5 phases
r2e-cache/TEST_DEVELOPMENT_PLAN.md         — 29 tests, 5 phases (+Phase 0 fix)
r2e-rate-limit/TEST_DEVELOPMENT_PLAN.md    — 36 tests, 7 phases
r2e-openapi/TEST_DEVELOPMENT_PLAN.md       — 40 tests, 7 phases
r2e-utils/TEST_DEVELOPMENT_PLAN.md         — 24 tests, 5 phases
r2e-test/TEST_DEVELOPMENT_PLAN.md          — 33 tests, 4 phases
r2e-cli/TEST_DEVELOPMENT_PLAN.md           — 63 tests, 7 phases
r2e-openfga/TEST_DEVELOPMENT_PLAN.md       — 31 tests, 6 phases
```
