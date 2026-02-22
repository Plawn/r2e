# r2e-security — Test Development Plan

## Current State

- **25 tests** (11 openid role extraction, 14 keycloak role extraction)
- **Coverage**: ~30% — role extractors well-tested, core JWT/JWKS/extractor untested
- **Gap**: JWT validation, JWKS cache, bearer token extraction, SecurityError, AuthenticatedUser helpers all at 0 tests

---

## Phase 1: SecurityError Response Mapping (Quick Win)

**File**: `src/error.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `missing_auth_header_401` | `SecurityError::MissingAuthHeader` → 401 status |
| `invalid_auth_scheme_401` | `SecurityError::InvalidAuthScheme` → 401 |
| `invalid_token_401` | `SecurityError::InvalidToken(msg)` → 401 with message |
| `token_expired_401` | `SecurityError::TokenExpired` → 401 |
| `unknown_key_id_401` | `SecurityError::UnknownKeyId(kid)` → 401 |
| `jwks_fetch_error_500` | `SecurityError::JwksFetchError(msg)` → 500 |
| `validation_failed_401` | `SecurityError::ValidationFailed(msg)` → 401 |
| `display_formatting` | Each variant's `Display` output matches expected |
| `into_app_error` | `SecurityError` → `HttpError` conversion |
| `json_body_format` | Response body is JSON `{"error": "..."}` |

---

## Phase 2: Bearer Token Extraction (Pure Logic)

**File**: `src/extractor.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `valid_bearer_token` | `"Bearer abc123"` → `Ok("abc123")` |
| `case_insensitive_scheme` | `"bearer abc123"` → `Ok("abc123")` |
| `missing_authorization_header` | No header → `Err(MissingAuthHeader)` |
| `invalid_scheme_basic` | `"Basic abc123"` → `Err(InvalidAuthScheme)` |
| `empty_authorization_header` | `""` → `Err(InvalidAuthScheme)` |
| `bearer_only_no_token` | `"Bearer "` → `Err(InvalidToken)` or empty string |
| `extra_whitespace` | `"Bearer   abc123"` → `Ok("abc123")` (trimmed) |
| `token_with_dots` | `"Bearer eyJ.eyJ.sig"` → `Ok("eyJ.eyJ.sig")` |

---

## Phase 3: AuthenticatedUser Helpers

**File**: `src/identity.rs` — add `#[cfg(test)] mod tests`

### 3.1 Construction from Claims

| Test | Description |
|------|-------------|
| `from_claims_complete` | Claims with sub, email, roles → all fields populated |
| `from_claims_missing_sub` | No `sub` claim → fallback to `"unknown"` |
| `from_claims_missing_email` | No `email` claim → `None` |
| `from_claims_empty_roles` | No roles in claims → empty vec |
| `from_claims_with_custom_extractor` | `from_claims_with()` uses custom `RoleExtractor` |

### 3.2 Role Checking Methods

| Test | Description |
|------|-------------|
| `has_role_present` | `has_role("admin")` → `true` when role exists |
| `has_role_absent` | `has_role("superadmin")` → `false` |
| `has_role_case_sensitive` | `has_role("Admin")` → `false` (exact match) |
| `has_any_role_one_match` | `has_any_role(&["admin", "editor"])` → `true` (admin present) |
| `has_any_role_none_match` | `has_any_role(&["superadmin"])` → `false` |
| `has_any_role_empty_slice` | `has_any_role(&[])` → `false` |

### 3.3 Identity Trait

| Test | Description |
|------|-------------|
| `identity_sub` | `Identity::sub()` returns correct subject |
| `identity_roles` | `Identity::roles()` returns role list |
| `identity_email` | `Identity::email()` returns `Some(email)` |
| `identity_claims` | `Identity::claims()` returns raw JSON value |

---

## Phase 4: JWT Validation with Static Keys

**File**: `src/jwt.rs` — add `#[cfg(test)] mod tests`

Uses `r2e-test`'s `TestJwt` to create tokens with known static keys.

### 4.1 `JwtClaimsValidator`

| Test | Description |
|------|-------------|
| `validate_valid_token` | Well-formed token with correct signature → `Ok(claims)` |
| `validate_expired_token` | Token with past `exp` → `Err(TokenExpired)` |
| `validate_invalid_signature` | Tampered token → `Err(InvalidToken)` |
| `validate_wrong_issuer` | Mismatched issuer → `Err(ValidationFailed)` |
| `validate_wrong_audience` | Mismatched audience → `Err(ValidationFailed)` |
| `validate_malformed_token` | `"not.a.jwt"` → `Err(InvalidToken)` |
| `validate_empty_token` | `""` → `Err(InvalidToken)` |

### 4.2 `JwtValidator` with Identity Builder

| Test | Description |
|------|-------------|
| `validate_returns_authenticated_user` | Valid token → `AuthenticatedUser` with sub/email/roles |
| `validate_with_custom_identity_builder` | Custom builder produces custom identity type |
| `validate_claims_passthrough` | `validate_claims()` returns raw claims |
| `claims_validator_accessor` | `.claims_validator()` returns inner validator |
| `config_accessor` | `.config()` returns SecurityConfig |

---

## Phase 5: SecurityConfig

**File**: `src/config.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `config_new_required_fields` | `SecurityConfig::new(issuer, audience, jwks_url)` stores fields |
| `config_with_cache_ttl` | `.with_cache_ttl(300)` sets TTL |
| `config_fields_accessible` | All fields accessible via getters |

---

## Phase 6: JWKS Cache (Requires HTTP Mocking)

**File**: `src/jwks.rs` — add `#[cfg(test)] mod tests`

Requires: `wiremock` or `mockito` as dev-dependency for mocking JWKS endpoints.

| Test | Description |
|------|-------------|
| `get_key_found` | Known `kid` → returns `DecodingKey` |
| `get_key_missing_triggers_refresh` | Unknown `kid` → fetch JWKS → returns key |
| `get_key_still_missing_after_refresh` | Unknown `kid` after refresh → `Err(UnknownKeyId)` |
| `refresh_parses_jwks_response` | Valid JWKS JSON → keys cached |
| `refresh_network_error` | HTTP failure → `Err(JwksFetchError)` |
| `refresh_malformed_response` | Invalid JSON → `Err(JwksFetchError)` |
| `refresh_empty_keys` | JWKS with `{"keys": []}` → no keys cached |
| `refresh_skips_keys_without_kid` | Keys missing `kid` field → skipped |
| `rsa_key_reconstruction` | RSA `n`/`e` components → valid `DecodingKey` |
| `unsupported_key_type` | Non-RSA key → skipped with warning |
| `concurrent_get_key_safe` | Multiple threads calling `get_key()` → no race |

---

## Phase 7: Integration Tests

**File**: `example-app/tests/security_test.rs` (new)

| Test | Description |
|------|-------------|
| `expired_token_rejected` | Expired JWT → 401 |
| `tampered_token_rejected` | Modified payload → 401 |
| `malformed_token_rejected` | Non-JWT string → 401 |
| `optional_identity_none` | No Authorization header → `Option<AuthenticatedUser>` = None |
| `optional_identity_some` | Valid token → `Option<AuthenticatedUser>` = Some |
| `optional_identity_invalid_errors` | Invalid token → error (not None) |
| `multiple_roles_accessible` | Token with ["admin", "user"] → both roles available |
| `custom_role_extractor` | Keycloak-style claims → roles extracted correctly |

---

## Phase 8: TestJwt Enhancements (in r2e-test)

**File**: `r2e-test/src/jwt.rs` — extend API and add tests

### New Methods

| Method | Description |
|--------|-------------|
| `token_expired()` | Generate token with `exp` in the past |
| `token_not_yet_valid()` | Generate token with `nbf` in the future |
| `token_with_custom_claims(map)` | Arbitrary claim injection |
| `token_without_kid()` | Token with no `kid` in header |

### Self-Tests

| Test | Description |
|------|-------------|
| `test_jwt_token_structure` | Generated token has 3 dot-separated parts |
| `test_jwt_claims_contain_sub` | Token claims include configured `sub` |
| `test_jwt_claims_contain_roles` | Token claims include configured roles |
| `test_jwt_claims_contain_email` | Token claims include configured email |
| `test_jwt_validator_accepts_own_tokens` | `TestJwt::validator()` validates its own tokens |
| `test_jwt_expired_token_rejected` | Expired token → validator rejects |
| `test_jwt_different_key_rejected` | Token from different TestJwt → rejected |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 10 | 1h | None |
| Phase 2 | 8 | 1h | None |
| Phase 3 | 16 | 2h | serde_json fixtures |
| Phase 4 | 12 | 3h | r2e-test TestJwt |
| Phase 5 | 3 | 30m | None |
| Phase 6 | 11 | 4h | wiremock/mockito |
| Phase 7 | 8 | 2h | TestJwt enhancements |
| Phase 8 | 11 | 2h | None |
| **Total** | **79** | **~15.5h** | |
