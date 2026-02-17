# r2e-core — Test Development Plan

## Current State

- **27 tests** (12 beans, 9 config, 6 secrets)
- **Coverage**: ~5% — only DI and configuration subsystems tested
- **Gap**: AppBuilder, guards, plugins, error handling, managed resources, health checks, interceptors, HTTP, WebSocket, SSE, multipart — all 0 tests

---

## Phase 1: Error Handling & Types (Quick Wins)

### 1.1 `AppError` Response Mapping

**File**: `src/error.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `app_error_not_found_status` | `AppError::NotFound` → 404 |
| `app_error_bad_request_status` | `AppError::BadRequest` → 400 |
| `app_error_unauthorized_status` | `AppError::Unauthorized` → 401 |
| `app_error_forbidden_status` | `AppError::Forbidden` → 403 |
| `app_error_internal_status` | `AppError::Internal` → 500 |
| `app_error_custom_status` | `AppError::Custom(code, msg)` → custom code |
| `app_error_json_body_format` | Response body is `{"error": "..."}` JSON |
| `app_error_display_formatting` | `Display` trait produces expected strings |

### 1.2 `ManagedError` / `ManagedErr<E>` Wrappers

**File**: `src/managed.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `managed_error_into_response` | `ManagedError(AppError::...)` converts to correct HTTP response |
| `managed_err_wraps_custom_error` | `ManagedErr(MyError)` delegates to `IntoResponse` |
| `managed_err_preserves_status` | Status code is preserved through the wrapper |

---

## Phase 2: Guards System

### 2.1 `GuardContext` & `PathParams`

**File**: `src/guards.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `path_params_get_existing` | `PathParams::get("id")` returns `Some("123")` |
| `path_params_get_missing` | `PathParams::get("missing")` returns `None` |
| `path_params_from_hashmap` | Construction from `HashMap<String, String>` |
| `guard_context_method_name` | `ctx.method_name` is accessible |
| `guard_context_controller_name` | `ctx.controller_name` is accessible |
| `guard_context_identity_some` | `ctx.identity()` returns `Some` when present |
| `guard_context_identity_none` | `ctx.identity()` returns `None` for `NoIdentity` |
| `guard_context_identity_sub` | `ctx.identity_sub()` extracts subject |
| `guard_context_identity_roles` | `ctx.identity_roles()` extracts roles |
| `guard_context_identity_email` | `ctx.identity_email()` extracts email |
| `guard_context_uri_path` | `ctx.path()` returns URI path |
| `guard_context_query_string` | `ctx.query_string()` returns query params |

### 2.2 `NoIdentity` Sentinel

| Test | Description |
|------|-------------|
| `no_identity_sub` | `NoIdentity.sub()` returns expected value |
| `no_identity_roles` | `NoIdentity.roles()` returns empty vec |

---

## Phase 3: Plugin System

### 3.1 `DeferredAction` & `DeferredContext`

**File**: `src/plugin.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `deferred_action_creation` | `DeferredAction::new("name", \|ctx\| {...})` stores name |
| `deferred_context_add_layer` | `ctx.add_layer(layer)` appends to layer list |
| `deferred_context_store_data` | `ctx.store_data(value)` stores typed data |
| `deferred_context_on_serve` | `ctx.on_serve(f)` registers serve callback |
| `deferred_context_on_shutdown` | `ctx.on_shutdown(f)` registers shutdown callback |

### 3.2 Built-in Plugins

**File**: `src/plugins.rs` — add `#[cfg(test)] mod tests`

Requires constructing a minimal `axum::Router` for integration testing.

| Test | Description |
|------|-------------|
| `health_plugin_returns_200` | `GET /health` → 200 "OK" |
| `health_plugin_custom_path` | Custom health endpoint path |
| `cors_permissive_allows_origin` | CORS headers present in response |
| `error_handling_catches_panic` | Panic in handler → JSON 500 |
| `normalize_path_trailing_slash` | `/users/` redirects to `/users` |
| `plugin_should_be_last_warning` | Installing after "should_be_last" plugin logs warning |

---

## Phase 4: AppBuilder Integration Tests

### 4.1 State Building

**File**: `tests/builder_test.rs` (new integration test file)

| Test | Description |
|------|-------------|
| `build_state_with_beans` | `with_bean::<T>()` → state contains T |
| `build_state_with_async_bean` | `with_async_bean::<T>()` → async bean resolves |
| `build_state_with_producer` | `with_producer::<P>()` → P::Output in state |
| `build_state_with_provide` | `.provide(value)` → value available in state |
| `build_state_with_config` | `.with_config(config)` → config accessible |
| `build_state_missing_dependency` | Missing bean → panic with diagnostic |
| `build_state_circular_dependency` | Cycle → panic with diagnostic |

### 4.2 Plugin Lifecycle

| Test | Description |
|------|-------------|
| `pre_state_plugin_before_build` | `.plugin(P)` runs before `build_state()` |
| `post_state_plugin_after_build` | `.with(P)` runs after `build_state()` |
| `plugin_ordering_respected` | Plugins execute in registration order |
| `deferred_actions_execute_on_serve` | Serve hooks fire at startup |
| `shutdown_hooks_execute` | Shutdown hooks fire on stop |

### 4.3 Controller Registration

| Test | Description |
|------|-------------|
| `register_controller_adds_routes` | Routes appear in router |
| `register_controller_collects_metadata` | Route metadata accessible |
| `register_controller_discovers_consumers` | Consumer methods registered |
| `register_controller_discovers_scheduled` | Scheduled tasks collected |

### 4.4 End-to-End

| Test | Description |
|------|-------------|
| `full_builder_to_router` | `AppBuilder::new()...build()` produces working router |
| `startup_hook_runs_before_serving` | `on_start` callback executes |

---

## Phase 5: Health Checks

**File**: `src/health.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `health_indicator_up` | Indicator returning `Up` → status "UP" |
| `health_indicator_down` | Indicator returning `Down` → status "DOWN" |
| `aggregation_all_up` | All indicators up → overall "UP" |
| `aggregation_one_down` | One indicator down → overall "DOWN" |
| `readiness_vs_liveness` | Readiness filters indicators correctly |
| `health_cache_ttl` | Repeated calls within TTL return cached |
| `uptime_calculation` | Uptime increases over time |

---

## Phase 6: Interceptors

**File**: `src/interceptors.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `interceptor_around_executes_inner` | `around()` calls the inner function |
| `interceptor_context_provides_state` | `InterceptorContext` exposes state |
| `interceptor_context_provides_method` | Method name accessible |
| `cacheable_json_serialization` | `Cacheable` impl for `Json<T>` round-trips |
| `cacheable_json_deserialization` | Bytes → `Json<T>` reconstruction |

---

## Phase 7: Request ID

**File**: `src/request_id.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `generates_uuid_v4` | Generated ID matches UUID format |
| `propagates_existing_header` | Existing `X-Request-Id` header preserved |
| `injects_when_missing` | Missing header → new UUID injected |
| `extractor_from_request` | `RequestId` extractable from request parts |

---

## Phase 8: WebSocket & SSE (if feature-gated)

**File**: `src/ws.rs` / `src/sse.rs` — add `#[cfg(test)] mod tests`

These require `axum::test` utilities or `tower::ServiceExt` for simulated requests.

| Test | Description |
|------|-------------|
| `ws_upgrade_negotiation` | WebSocket upgrade returns 101 |
| `sse_stream_sends_events` | SSE endpoint streams events |
| `sse_keep_alive_configured` | Keep-alive interval matches config |

---

## Phase 9: Secure Headers

**File**: `src/secure_headers.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `default_headers_injected` | Default security headers present (X-Frame-Options, X-Content-Type-Options, etc.) |
| `csp_header_format` | Content-Security-Policy header correctly formatted |
| `custom_header_override` | Custom configuration overrides defaults |
| `referrer_policy_set` | Referrer-Policy header present |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 11 | 1h | None |
| Phase 2 | 14 | 2h | None |
| Phase 3 | 11 | 3h | axum test utils |
| Phase 4 | 17 | 4h | Minimal controller fixtures |
| Phase 5 | 7 | 2h | None |
| Phase 6 | 5 | 1h | None |
| Phase 7 | 4 | 1h | axum test utils |
| Phase 8 | 3 | 2h | Feature flags, WS client |
| Phase 9 | 4 | 1h | None |
| **Total** | **76** | **~17h** | |
