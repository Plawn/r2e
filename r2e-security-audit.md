# Security Audit — `r2e-security`

Audit of the JWT/OIDC security crate (`r2e-security/src/*.rs`).
Scope: token validation, JWKS cache, identity extraction, role guards.

## Summary

The crate is well-structured and the core JWT validation logic is sound —
algorithm allowlisting, issuer/audience/exp/nbf enforcement, and client-facing
error messages that don't leak internals. No exploitable signature-bypass was
found under default configuration. The findings below are mostly about
**resilience and hardening** rather than a direct authentication bypass.

## What's done right

- **Algorithm confusion is mitigated** (`jwt.rs:82-92`): the token's `alg` is
  checked against `allowed_algorithms` (default `RS256` only) *before* decoding,
  and `validation.algorithms` is also pinned. `alg:none` and HS256-with-RSA-pubkey
  attacks are blocked under defaults. Covered by `tests/jwt.rs:113-140`.
- **No info leak to clients** (`error.rs:46-56`): every `SecurityError` renders as
  `401 {"error":"Unauthorized"}`; details only go to `warn!`/`debug!` logs.
- **Refresh storms are bounded** (`jwks.rs:218-250`): double-checked locking plus
  `jwks_min_refresh_interval_secs` rate-limits refreshes, so a flood of requests
  with bogus `kid`s triggers at most one fetch per interval.

## Findings

| # | Severity | Title | Location |
|---|----------|-------|----------|
| 1 | Medium (DoS) | ✅ **Fixed** — No HTTP timeout on the JWKS client | `jwks.rs:124` |
| 2 | Medium (availability) | ✅ **Fixed** — Stale-but-valid keys not used as fallback on refresh failure | `jwks.rs:143-175` |
| 3 | Low/Medium (hardening) | ✅ **Fixed** — `jwks_url` scheme not constrained to HTTPS | `config.rs`, `jwks.rs:181` |
| 4 | Low (DoS) | ✅ **Fixed** — No response-size limit on JWKS fetch | `jwks.rs:190-193` |
| 5 | Low | ✅ **Fixed** — Tokens with missing/empty `sub` validate successfully | `identity.rs:308-312` |
| 6 | Low (footgun) | ✅ **Fixed** — `AuthenticatedUser` derives `Deserialize` | `identity.rs:227` |
| 7 | Low (hardening) | ✅ **Fixed** — JWK `kty`/`alg` not cross-checked against token `alg` | `jwks.rs:58-100` |
| 8 | Informational | ✅ **Fixed** — `JwksFetchError` maps to `401 Unauthorized` (now `503`) | `error.rs:58-62` |
| 9 | Low (hygiene) | ✅ **Fixed** — Inline `#[cfg(test)] mod tests` violates project convention | `jwks.rs:267-361` |
| 10 | Informational | ✅ **Fixed** — `extract_bearer_token` keeps leading whitespace | `extractor.rs:13` |

---

### 1. No HTTP timeout on the JWKS client — Medium (DoS)

`jwks.rs:124` uses `reqwest::Client::new()` with no `.timeout()` /
`.connect_timeout()`. `get_key()` is awaited **inline during request
authentication**, so a slow or hung JWKS endpoint stalls auth indefinitely (and
`JwksCache::new` blocks startup). A degraded IdP becomes a hang for the service.

**Fix:** build the client with explicit timeouts, e.g.
`Client::builder().timeout(Duration::from_secs(...)).connect_timeout(...).build()`.

### 2. Stale-but-valid keys not used as fallback on refresh failure — Medium (availability)

In `get_key` (`jwks.rs:143-175`), when a cached key exists but the TTL has
expired, `try_refresh(false)` is called and its error is propagated with `?`. If
the IdP is briefly unreachable, authentication fails for *all* requests — even
though the cached public key would still verify tokens correctly. Public signing
keys rotate on the order of days/weeks; a few-second IdP blip should not take
down auth.

**Fix:** on refresh failure, fall back to the existing cached key; only hard-fail
when the `kid` is genuinely absent from the cache.

### 3. `jwks_url` scheme not constrained to HTTPS — Low/Medium (hardening)

`config.rs` / `jwks.rs:181` fetch whatever URL is configured. A misconfigured
`http://` JWKS URL allows a network MITM to substitute signing keys and forge
tokens.

**Fix:** reject non-HTTPS `jwks_url` (with an explicit opt-out for local dev).

### 4. No response-size limit on JWKS fetch — Low (DoS)

`jwks.rs:190-193` calls `response.json()` with no cap. A compromised/hostile
JWKS endpoint can return an unbounded body and exhaust memory.

**Fix:** read with a byte limit (e.g. bounded `bytes()` read) before parsing.

### 5. Tokens with missing/empty `sub` validate successfully — Low

`build_authenticated_user` (`identity.rs:308-312`) defaults `sub` to `""` when
absent. A signature-valid token with no `sub` yields
`AuthenticatedUser { sub: "" }`. Any authorization keyed on `sub` could misbehave
or collide.

**Fix:** reject tokens lacking a non-empty `sub` claim.

### 6. `AuthenticatedUser` derives `Deserialize` — Low (footgun)

`identity.rs:227` makes the identity type deserializable, including its `roles`
and raw `claims`. If it is ever used as a request-body extractor
(`Json<AuthenticatedUser>`), a client fully controls their own identity and
roles. The derive is presumably for response serialization.

**Fix:** split into separate input/output types, or drop `Deserialize`.

### 7. JWK `kty`/`alg` not cross-checked against the token's `alg` — Low (hardening)

`jwks.rs:58-100` builds a `DecodingKey` purely from `kty`, ignoring the JWK's own
`alg`. Harmless under the default single-algorithm config (jsonwebtoken's
key-type check covers the rest), but if a user enables multiple algorithm
families this skips a defense-in-depth check.

**Fix:** match the JWK's declared `alg`/`kty` against the requested algorithm, or
document the assumption.

### 8. `JwksFetchError` maps to `401 Unauthorized` — Informational

`error.rs:58-62` collapses every error to `Unauthorized`. A JWKS fetch/parse
failure is a server-side (5xx) condition, not a client auth failure.
Security-safe, but misleading for clients and observability.

### 9. Inline `#[cfg(test)] mod tests` violates project convention — Low (hygiene)

`jwks.rs:267-361` uses `#[cfg(test)] mod tests { ... }`, which the project's
`CLAUDE.md` explicitly forbids ("Tests live in `<crate>/tests/`… do NOT use
`#[cfg(test)] mod tests`").

**Fix:** move to `tests/jwks.rs`, adding `pub`/`#[doc(hidden)]` accessors for
`CachedJwk`, `is_stale`, `can_attempt`.

### 10. `extract_bearer_token` keeps leading whitespace — Informational

`extractor.rs:13` uses `splitn(2, ' ')`, so `"Bearer  <tok>"` (double space)
yields a token with a leading space (rejected downstream as malformed — benign,
but sloppy).

---

## Status

All 10 findings have been addressed.

- **#1** JWKS client now built with request/connect timeouts (`SecurityConfig::with_request_timeout` / `with_connect_timeout`).
- **#2** Stale-but-valid cached keys are reused when a refresh fails; only a genuinely unknown `kid` hard-fails.
- **#3** Non-HTTPS `jwks_url` rejected unless `SecurityConfig::allow_insecure_jwks_url()` is set.
- **#4** JWKS response body bounded by `jwks_max_response_bytes` (default 1 MiB), checked via `Content-Length` and streamed.
- **#5** Tokens without a non-empty `sub` claim are rejected by `JwtClaimsValidator::validate`.
- **#6** `Deserialize` removed from `AuthenticatedUser` (and the example `DbUser`/`TenantUser` identities) so a trusted identity can never be built from a request body.
- **#7** `kty`/`alg` of the resolved JWK are cross-checked against the token's algorithm (defense-in-depth against algorithm confusion).
- **#8** `JwksFetchError` now surfaces as `503 Service unavailable` instead of `401`.
- **#9** Inline `#[cfg(test)] mod tests` moved to `tests/jwks.rs` (internals exposed via `#[doc(hidden)] pub`).
- **#10** `extract_bearer_token` trims surrounding whitespace.

All `r2e-security` tests and the affected example crates pass; the full workspace compiles.
