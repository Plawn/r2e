# r2e-macros — Test Development Plan

## Coverage Gaps (llvm-cov 2026-07-21)

- **Line coverage: 84.9% (7234/8517)**
- **Function coverage: 89.0% (690/775)**

### Per-file uncovered lines (sorted by gap)

| File | Covered | Total | Line% | Uncovered | Key uncovered code paths |
|---|---|---|---|---|---|
| `api_error_derive.rs` | 399 | 524 | 76.1% | 125 | Named-field variants in Display/IntoResponse/From codegen (multi-unnamed, `#[from]` on named, humanized fallback) |
| `bean_attr.rs` | 638 | 749 | 85.2% | 111 | `BeanConsumerMethod`/`BeanScheduledMethod`/`BeanInterceptedMethod` struct construction; sync `Bean` trait impl (vs async path); `register_call` sync branch |
| `main_attr.rs` | 239 | 341 | 70.1% | 102 | `OrderedHooks` (ordered test barrier/poison logic); `expand_test` runtime builder codegen; `parse_bool`/`parse_int`/`parse_str` helpers; `current_thread` flavor branch |
| `config_derive.rs` | 424 | 513 | 82.7% | 89 | `FieldInfo` struct; `extract_doc_comment`; `Option<T>` with env+default codegen; env-only `Option` branch; garde `Validate` call codegen; tagged-enum `ConfigProperties` impl (`from_config` tag dispatch, metadata, none_arm) |
| `codegen/controller_impl.rs` | 826 | 910 | 90.8% | 84 | `Controller<S,W>` trait impl body (construct/routes/register_meta/validate_config); route metadata builder (all fields); pre-auth guard middleware codegen with `PreAuthGuardContext` |
| `params_derive.rs` | 195 | 278 | 70.1% | 83 | `PrefixedExtract`/`FromRequestParts` impls; `ParamsMetadata` codegen; per-field extraction: optional path params, query with default expr, query required, header optional/required (full branch matrix) |
| `lib.rs` | 55 | 133 | 41.4% | 78 | Attribute-macro entry points (`#[controller]`, `#[routes]`, `#[bean]`, `#[grpc_routes]`, `#[main]`, `#[test]`) — thin dispatch, covered indirectly |
| `codegen/handlers.rs` | 845 | 911 | 92.8% | 66 | `generate_single_handler` case matrix; WebSocket handler codegen (pattern 2: method returns `impl WsHandler`, guard preflight, upgrade closure, deco items); handler-level extra params |
| `type_utils.rs` | 95 | 147 | 64.6% | 52 | `type_base_name` (non-Path fallback); `named_bean_newtype_ident`; `parse_inject_name` (bare `#[inject]`, `#[inject(identity)]` skip, `#[inject(name = "...")]`, unknown-arg error) |
| `grpc_routes_parsing.rs` | 75 | 118 | 63.6% | 43 | `GrpcRoutesArgs` parsing (descriptor, unknown arg error); `GrpcRoutesImplDef` fields; `extract_identity_param` (duplicate identity error, optional identity, attr stripping) |
| `bg_service_derive.rs` | 33 | 75 | 44.0% | 42 | `#[service(state)]` removed-attr error; unit struct vs named struct branches; field classification (#[inject]/#[config]/#[config_section]); `ServiceComponent` impl codegen; `generate_unit_impl` |
| `extract/scheduled.rs` | 95 | 133 | 71.4% | 38 | Overlap policy parsing; skip_if parsing; `ScheduledConfig` struct construction from attrs |
| `routes_parsing.rs` | 420 | 455 | 92.3% | 35 | Various edge-case attribute parsing branches |
| `extract/route.rs` | 214 | 253 | 84.6% | 39 | Summary/description/deprecated/tag extraction from doc comments |
| `extract/consumer.rs` | 80 | 112 | 71.4% | 32 | `ConsumerConfig` attr parsing (concurrency, batch_size, group, ack_mode) |
| `field_resolver.rs` | 74 | 100 | 70.0% | 31 | `FieldKind::Default` branch; unannotated-field error message with hints; `config_resolve_expr` Option branch (absent → None, mistyped → panic) |
| `producer_attr.rs` | 221 | 253 | 78.0% | 32 | Producer codegen for named-inject and config section fields |
| `from_multipart.rs` | 184 | 209 | 78.0% | 25 | `#[derive(FromMultipart)]` Vec/Option field codegen |
| `decorator_bean_derive.rs` | 167 | 187 | 81.0% | 20 | `#[derive(DecoratorBean)]` guard/interceptor trait impl |

### What tests would close these gaps

**1. `api_error_derive.rs` (125 uncov)** — compile-pass/integration tests exercising:
- Named-field variant with `#[from]` + extra fields (codegen: `From` impl + `Display` delegation)
- Multi-unnamed-field variant (humanized `Display` fallback, tuple bindings in `IntoResponse`)
- Single-named-String-field variant (uses the string field as error body)
- `std::error::Error::source()` on named `#[from]` variant

**2. `bean_attr.rs` (111 uncov)** — integration tests exercising:
- Sync `#[bean]` (non-async build fn) — the `Bean` trait impl vs `AsyncBean`
- `#[bean]` with `#[consumer]` method (BeanConsumerMethod path)
- `#[bean]` with `#[scheduled]` + `#[intercept]` (BeanInterceptedMethod path)
- `#[bean]` with `#[async_exec(executor = "field")]`

**3. `main_attr.rs` (102 uncov)** — compile-pass tests for:
- `#[r2e::test(order = N)]` ordered barrier codegen
- `#[r2e::test(order = N)]` with `#[should_panic]` (expected-panic-mark)
- `#[r2e::main(flavor = "current_thread")]`
- `#[r2e::main]` runtime config helpers (`parse_bool`, `parse_int`, `parse_str`)

**4. `config_derive.rs` (89 uncov)** — integration/compile-pass tests for:
- Tagged-enum `ConfigProperties` (tag dispatch, unknown tag error, missing tag with default)
- `Option<T>` field with `#[config(env = "...", default = ...)]` 
- `Option<T>` field with `#[config(env = "...")]` only
- `#[validate(...)]` fields triggering garde validation in `from_config`
- `#[config(section)]` nested struct with `register_children`

**5. `params_derive.rs` (83 uncov)** — integration tests for:
- Optional path parameter (`Option<T>` path)
- Query parameter with `#[param(default = expr)]`
- Required header parameter (missing → 400)
- Optional header parameter
- Nested `#[param(prefix = "...")]` struct (PrefixedExtract composition)

**6. `codegen/handlers.rs` (66 uncov)** — integration tests exercising:
- WebSocket handler returning `impl WsHandler` (pattern 2)
- WebSocket handler with guards (guard preflight → upgrade)
- Handler with `#[managed]` params and interceptors (Case 3)

**7. `bg_service_derive.rs` (42 uncov)** — compile-pass/fail tests for:
- `#[derive(BackgroundService)]` on unit struct
- `#[derive(BackgroundService)]` with `#[inject]` + `#[config]` fields
- `#[derive(BackgroundService)]` on enum → compile error
- `#[service(state = ...)]` → removed-attribute error

**8. `type_utils.rs` (52 uncov)** — unit tests for:
- `type_base_name` with path type, non-path type
- `named_bean_newtype_ident` → PascalCase composition
- `parse_inject_name` all branches: bare `#[inject]`, `#[inject(identity)]`, `#[inject(name = "x")]`, unknown arg error

**9. `grpc_routes_parsing.rs` (43 uncov)** — unit/compile tests for:
- `GrpcRoutesArgs` with `descriptor = expr`
- Unknown `#[grpc_routes]` argument → error
- `extract_identity_param`: duplicate identity → error, optional identity

**10. `field_resolver.rs` (31 uncov)** — compile-fail tests for:
- Unannotated field in bean/service context → error with hints
- `#[default]` field in context where `allow_default = false`
- `config_resolve_expr` with `Option<T>` (absent key → None)

---

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
| `config_properties_basic.rs` | Basic `#[derive(ConfigProperties)]` with typed fields |
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
