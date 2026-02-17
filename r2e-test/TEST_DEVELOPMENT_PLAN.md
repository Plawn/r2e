# r2e-test — Test Development Plan

## Current State

- **0 tests** (this is a test utility crate, heavily used by example-app but no self-tests)
- **Coverage**: 0% direct (used by 26+ integration tests indirectly)
- **Gap**: TestApp request building, TestResponse assertions, TestJwt token generation — no direct verification

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
