# r2e-macros — Test Development Plan

## Current State

- **65 tests** (36 compile via trybuild, 29 integration via example-app)
- **Coverage**: ~50% syntax validation, ~30% runtime behavior
- **Strengths**: Compile pass/fail tests are thorough for error diagnostics
- **Gaps**: Many macros have syntax validation but no runtime execution tests

---

## Phase 1: Expand Compile Tests for Untested Derives

### 1.1 `#[derive(FromMultipart)]`

**Dir**: `r2e-compile-tests/compile-pass/`

| Test File | Description |
|-----------|-------------|
| `from_multipart_basic.rs` | Basic struct with String/i64 fields |
| `from_multipart_optional.rs` | Struct with `Option<T>` fields |
| `from_multipart_file.rs` | Struct with `UploadedFile` field |
| `from_multipart_vec_files.rs` | Struct with `Vec<UploadedFile>` field |

**Dir**: `r2e-compile-tests/compile-fail/`

| Test File | Description |
|-----------|-------------|
| `from_multipart_enum.rs` | `#[derive(FromMultipart)]` on enum → error |
| `from_multipart_tuple.rs` | Tuple struct → error |

### 1.2 `#[derive(ConfigProperties)]`

**Dir**: `r2e-compile-tests/compile-pass/`

| Test File | Description |
|-----------|-------------|
| `config_properties_basic.rs` | `#[config(prefix = "db")]` with typed fields |
| `config_properties_defaults.rs` | `#[config(default = 10)]` on fields |
| `config_properties_optional.rs` | `Option<T>` fields |
| `config_properties_nested.rs` | Nested config sections |

### 1.3 `#[derive(BeanState)]`

**Dir**: `r2e-compile-tests/compile-pass/`

| Test File | Description |
|-----------|-------------|
| `bean_state_basic.rs` | Basic state struct with `FromRef` generation |
| `bean_state_skip.rs` | `#[bean_state(skip_from_ref)]` attribute |

### 1.4 `#[derive(Cacheable)]`

**Dir**: `r2e-compile-tests/compile-pass/`

| Test File | Description |
|-----------|-------------|
| `cacheable_basic.rs` | Basic struct with `Serialize + Deserialize` |

---

## Phase 2: Runtime Tests for WebSocket & SSE

### 2.1 WebSocket Handler

**File**: `example-app/tests/ws_test.rs` (new)

Requires: `tokio-tungstenite` as dev-dependency in example-app.

| Test | Description |
|------|-------------|
| `ws_upgrade_success` | `GET /ws` with upgrade headers → 101 Switching Protocols |
| `ws_message_echo` | Send text message → receive response |
| `ws_binary_message` | Send binary frame → receive binary response |
| `ws_connection_close` | Client close → server handles gracefully |
| `ws_with_auth` | `#[roles("user")]` on WS → requires valid JWT in query/header |
| `ws_unauthorized` | Missing token → 401 before upgrade |

### 2.2 SSE Handler

**File**: `example-app/tests/sse_test.rs` (new)

| Test | Description |
|------|-------------|
| `sse_content_type` | Response has `text/event-stream` content-type |
| `sse_event_format` | Events follow `data: ...\n\n` format |
| `sse_keep_alive` | Keep-alive comments sent at configured interval |
| `sse_stream_completes` | Finite stream ends correctly |

---

## Phase 3: Runtime Tests for Managed Resources & Transactions

### 3.1 `#[managed]` Lifecycle

**File**: `example-app/tests/managed_test.rs` (new)

Requires: SQLite in-memory database.

| Test | Description |
|------|-------------|
| `managed_tx_commit_on_success` | Handler returns Ok → transaction committed |
| `managed_tx_rollback_on_error` | Handler returns Err → transaction rolled back |
| `managed_resource_acquired_before_handler` | Resource available in handler body |
| `managed_resource_released_after_handler` | Resource released even on panic |
| `managed_multiple_resources` | Two `#[managed]` params in same handler |

### 3.2 `#[transactional]` Decorator

**File**: `example-app/tests/transactional_test.rs` (new)

| Test | Description |
|------|-------------|
| `transactional_commit` | Successful handler → data persisted |
| `transactional_rollback` | Error in handler → data not persisted |
| `transactional_custom_pool` | `#[transactional(pool = "read_db")]` uses correct pool |

---

## Phase 4: Runtime Tests for Consumers & Scheduling

### 4.1 Event Consumers

**File**: `example-app/tests/consumer_test.rs` (new)

| Test | Description |
|------|-------------|
| `consumer_receives_event` | Emit event → consumer handler invoked |
| `consumer_receives_correct_type` | Only matching event type dispatched |
| `consumer_multiple_on_same_event` | Multiple consumers all receive event |
| `consumer_accesses_injected_state` | Consumer can use `#[inject]` fields |

### 4.2 Scheduled Tasks

**File**: `example-app/tests/scheduled_test.rs` (new)

| Test | Description |
|------|-------------|
| `scheduled_interval_runs` | `#[scheduled(every = 1)]` task runs at least twice in 3s |
| `scheduled_cancellation_stops` | Cancel token → task stops running |
| `scheduled_with_delay` | `#[scheduled(every = 2, delay = 1)]` first run after 1s |
| `scheduled_cron_parses` | `#[scheduled(cron = "* * * * * *")]` runs every second |
| `scheduled_error_doesnt_crash` | Task returning `Err` → logged, scheduler continues |

---

## Phase 5: Expand HTTP Verb Coverage

### 5.1 PUT / DELETE / PATCH

**File**: extend `example-app/tests/user_controller_test.rs`

| Test | Description |
|------|-------------|
| `put_update_user` | `PUT /users/{id}` with JSON body → 200 |
| `put_update_not_found` | `PUT /users/999` → 404 |
| `delete_user` | `DELETE /users/{id}` → 200/204 |
| `delete_not_found` | `DELETE /users/999` → 404 |
| `patch_partial_update` | `PATCH /users/{id}` with partial body → 200 |

---

## Phase 6: Guards & Middleware Runtime

### 6.1 Custom Guards

**File**: `example-app/tests/guard_test.rs` (new)

| Test | Description |
|------|-------------|
| `custom_guard_allows` | Guard returning `Ok(())` → handler executes |
| `custom_guard_rejects` | Guard returning `Err(response)` → short-circuits with error |
| `guard_receives_identity` | Guard context has correct identity info |
| `guard_receives_headers` | Guard context has request headers |
| `guard_receives_uri` | Guard context has correct URI/path |
| `pre_guard_runs_before_jwt` | `#[pre_guard]` fires before token extraction |
| `pre_guard_rejects_early` | Pre-guard rejection → no JWT validation attempted |

### 6.2 Tower Middleware & Layers

| Test | Description |
|------|-------------|
| `middleware_wraps_handler` | `#[middleware(my_fn)]` runs around handler |
| `layer_applied_to_route` | `#[layer(expr)]` layer present in route |

---

## Phase 7: Config Injection Edge Cases

### 7.1 Config Types

**File**: extend `example-app/tests/` or add compile tests

| Test | Description |
|------|-------------|
| `config_string_injection` | `#[config("key")]` → `String` field populated |
| `config_i64_injection` | `#[config("key")]` → `i64` field populated |
| `config_f64_injection` | `#[config("key")]` → `f64` field populated |
| `config_bool_injection` | `#[config("key")]` → `bool` field populated |
| `config_option_some` | `#[config("key")]` → `Option<String>` = `Some(...)` |
| `config_option_none` | Missing key → `Option<String>` = `None` |
| `config_missing_required_panics` | Missing key for non-Option → panic with env var hint |

---

## Phase 8: Optional Identity

### 8.1 Mixed Controller Pattern

**File**: `example-app/tests/mixed_controller_test.rs` (new)

| Test | Description |
|------|-------------|
| `public_endpoint_no_token` | Public endpoint works without JWT |
| `protected_endpoint_with_token` | `#[inject(identity)]` param endpoint works with JWT |
| `protected_endpoint_no_token` | Missing JWT on protected endpoint → 401 |
| `optional_identity_with_token` | `Option<AuthenticatedUser>` = `Some(user)` |
| `optional_identity_without_token` | `Option<AuthenticatedUser>` = `None` |
| `optional_identity_invalid_token` | Invalid JWT → error (not `None`) |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 12 compile | 2h | trybuild |
| Phase 2 | 10 | 4h | tokio-tungstenite |
| Phase 3 | 8 | 3h | SQLite in-memory |
| Phase 4 | 9 | 3h | Timing-sensitive |
| Phase 5 | 5 | 1h | None |
| Phase 6 | 9 | 3h | Custom guard fixtures |
| Phase 7 | 7 | 1h | Config fixtures |
| Phase 8 | 6 | 2h | TestJwt |
| **Total** | **66** | **~19h** | |
