# r2e-test — Test Development Plan

## Coverage Gaps (llvm-cov 2026-07-21)

- **Line coverage**: 69.4% (947/1365)
- **Function coverage**: 65.4% (155/237)

| File | Covered | Total | Line % | Uncovered |
|------|---------|-------|--------|-----------|
| `src/app.rs` | 391 | 638 | 61.3% | 247 |
| `src/session.rs` | 30 | 116 | 25.9% | 86 |
| `src/ws.rs` | 49 | 84 | 58.3% | 35 |
| `src/jwt.rs` | 126 | 147 | 85.7% | 21 |
| `src/ordering.rs` | 151 | 159 | 95.0% | 8 |
| `src/sse.rs` | 23 | 31 | 74.2% | 8 |
| `src/boot.rs` | 24 | 31 | 77.4% | 7 |
| `src/server.rs` | 37 | 43 | 86.0% | 6 |

### `src/app.rs` — 247 uncovered lines

Uncovered code paths:

| Lines | Code path | Missing test |
|-------|-----------|--------------|
| L180-237 | `RequestBuilder::form()`, `cookie()`, `query()`, `queries()`, `content_type()`, `file()` | Test request builder with form-encoded body, cookies, query params, content-type override, multipart file |
| L601-652 | `json_type_name()`, `json_shape_errors()` recursive matching (object/array/type mismatch branches) | Test `assert_json_shape` with nested objects, arrays with typed elements, type mismatches at depth |
| L752-835 | `assert_json_path()`, `assert_json_path_fn()`, `assert_json_contains()`, `assert_json_path_contains()`, `assert_json_shape()` on TestResponse | Test all JSON assertion methods on a TestResponse (not just the free functions) |
| L840-875 | `assert_header()`, `assert_header_exists()`, `json_path::<T>()` deserialization | Test header assertions (match, exists, missing) and typed json_path extraction |
| L923-962 | `bytes()`, `content_type()`, `is_json()`, `json_optional()`, `assert_content_type()`, `sse_events()` | Test response body accessors: raw bytes, content-type detection, optional JSON, SSE parsing |

### `src/session.rs` — 86 uncovered lines

Uncovered code paths:

| Lines | Code path | Missing test |
|-------|-----------|--------------|
| L37-53 | `with_bearer()`, `as_user()` | Test session-level auth: bearer header applied to all requests |
| L55-76 | `with_default_header()`, `set_cookie()`, `remove_cookie()`, `clear_cookies()`, `cookie()` | Test session cookie jar CRUD and default headers |
| L83-99 | `get()`, `post()`, `put()`, `patch()` request builders | Test that session requests inherit cookies and default headers |

### `src/ws.rs` — 35 uncovered lines

Uncovered code paths:

| Lines | Code path | Missing test |
|-------|-----------|--------------|
| L39-49 | `WsTestError` Display impls (Timeout/Closed/Protocol/Json) | Test error Display formatting for all variants |
| L86-99 | `send_binary()`, `close()` | Test binary message send + explicit close |
| L125-151 | `next_binary()`, `assert_no_message()` | Test binary receive with timeout + assert_no_message |

### `src/jwt.rs` — 21 uncovered lines

Uncovered code paths:

| Lines | Code path | Missing test |
|-------|-----------|--------------|
| L27-46 | `with_config()`, `token()`, `token_with_claims()` convenience methods | Test custom issuer/audience config + email claim in token |
| L99-125 | `Default for TestJwt`, `Expiration` enum, `TokenBuilder` fields | Test `TestJwt::default()`, token builder chain with all fields set |

---

## Phase 1: TestJwt Token Generation

**File**: `src/jwt.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `token_has_three_parts` | Generated token has 3 dot-separated segments |
| `token_contains_sub` | Decoded claims contain configured `sub` |
| `token_contains_email` | Decoded claims contain configured `email` |
| `token_contains_roles` | Decoded claims contain configured `roles` |
| `token_has_future_exp` | `exp` claim is in the future |
| `token_has_iat` | `iat` claim is present and recent |
| `validator_accepts_own_tokens` | `TestJwt::validator()` validates tokens from same instance |
| `claims_validator_accepts_own_tokens` | `TestJwt::claims_validator()` validates tokens from same instance |
| `different_key_rejects_token` | Token from one `TestJwt` → rejected by another instance's validator |
| `token_with_custom_claims` | `token_with_claims(map)` includes additional claims |
| `default_sub` | No explicit sub → uses default value |
| `default_roles_empty` | No explicit roles → empty vec |

---

## Phase 2: TestApp Request Building

**File**: `src/app.rs` or `src/lib.rs` — add `#[cfg(test)] mod tests`

Requires: A minimal `axum::Router` fixture for testing.

| Test | Description |
|------|-------------|
| `get_sends_get_request` | `app.get("/path")` → GET method received by handler |
| `post_json_sends_post` | `app.post_json("/path", body)` → POST with JSON content-type |
| `put_json_authenticated_sends_put` | `app.put_json_authenticated(...)` → PUT with Authorization header |
| `delete_authenticated_sends_delete` | `app.delete_authenticated(...)` → DELETE with Authorization header |
| `get_authenticated_includes_bearer` | Authorization header is `"Bearer <token>"` |
| `custom_header_sent` | Custom headers included in request |

---

## Phase 3: TestResponse Assertions

| Test | Description |
|------|-------------|
| `assert_ok_on_200` | 200 response → `assert_ok()` passes |
| `assert_ok_on_404` | 404 response → `assert_ok()` panics |
| `assert_created_on_201` | 201 → passes |
| `assert_bad_request_on_400` | 400 → passes |
| `assert_unauthorized_on_401` | 401 → passes |
| `assert_forbidden_on_403` | 403 → passes |
| `assert_not_found_on_404` | 404 → passes |
| `assert_status_custom` | `assert_status(418)` on 418 → passes |
| `json_deserialization` | `.json::<T>()` deserializes response body |
| `text_extraction` | `.text()` returns response body as string |
| `json_on_non_json_body` | `.json::<T>()` on non-JSON → meaningful error |

---

## Phase 4: New TestJwt Methods (API Extensions)

Add these methods and test them.

| Method + Test | Description |
|---------------|-------------|
| `token_expired()` + test | Generate token with `exp` in the past → validator rejects |
| `token_not_yet_valid()` + test | Generate token with `nbf` in the future → validator rejects |
| `token_without_kid()` + test | Generate token with no `kid` in header |
| `token_with_invalid_signature()` + test | Return a tampered token string |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 12 | 2h | jsonwebtoken |
| Phase 2 | 6 | 2h | axum Router fixture |
| Phase 3 | 11 | 1.5h | axum Router fixture |
| Phase 4 | 4 | 1.5h | None |
| **Total** | **33** | **~7h** | |
