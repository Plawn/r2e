# Plan — Containing the JSON codec (`serde_json`) behind an R2E façade

Follow-up to `plans/runtime-http-dependency-containment.md`. That plan put the
async runtime (`r2e-rt`) and the HTTP backend (`r2e-http`) behind R2E-owned
seams. This one does the same for the JSON codec, with one deliberate
difference in shape (see §1.3).

Branch: `task/json-containment`. Started 2026-08-24.

## 1. Where we actually are

### 1.1 The hot path is not ours

`Json<T>` — extractor *and* response — is `axum::Json`, re-exported as-is from
`r2e-http/src/lib.rs`. Axum hard-codes `serde_json::from_slice` /
`serde_json::to_writer` inside it (through `serde_path_to_error`). While that
type is axum's, no alternative codec can be plugged in anywhere, whatever the
rest of the workspace does. This is the actual coupling point.

### 1.2 Free-standing (de)serialization calls — ~60 sites

Mechanically replaceable, and they are where a faster codec pays:

| crate | sites | what |
|---|---|---|
| `r2e-events` + backends | 21 × `to_vec`, 4 × `from_slice` | event payloads — a real hot path on distributed backends |
| `r2e-core` | `error.rs` (`to_vec` error body), `ws.rs` / `sse.rs` (`to_string` per message), `decorators/interceptors.rs` (`Cacheable` for `Json<T>`) | |
| `r2e-macros` | `#[derive(Cacheable)]` emits `to_vec` / `from_slice`; `r2e-core` already does `pub use serde_json;` so generated code goes through it | an embryo of a façade already exists |
| `r2e-security` | `from_slice` on the JWT payload (via `jsonwebtoken`, which is serde_json-bound internally — see §5) | |

### 1.3 `serde_json::Value` as a *data type* — NOT a perf point, NOT abstracted

`r2e-security` (JWT claims, 31 occurrences), `guards.rs` (`identity_claims()`),
`builtins/health.rs`, `di/meta.rs`, `config/value.rs`, OpenAPI / multipart
schemas. These carry an open-ended JSON tree as *data*. Fast codecs
(`sonic-rs`, `simd-json`) win on typed `Serialize` / `Deserialize` structs, not
on a dynamic tree, and they each bring their own `Value` type — abstracting
`Value` would cost a trait or newtype over every access for no measurable gain.

**Decision:** `serde_json::Value` and `json!` stay `serde_json`, and
`serde_json` stays in the dependency graph as the *dynamic-tree* crate. The
boundary is about **who performs the (de)serialization work**, not about the
name of the tree type. The boundary check (§4) therefore counts only
`serde_json::(to_|from_)…` calls, never `Value`.

The one place where `Value`-as-data is genuinely a design smell — JWT claims in
`r2e-security` — is handled on its own merits in §5 (typed `StandardClaims`),
because deserializing straight into a struct removes the tree *and* the
per-request navigation, which is both cleaner and cheaper.

## 2. Phase 0 — Measure before moving

`r2e-http/benches/json.rs` (criterion): `to_vec` / `from_slice` on a ~4 KiB
`Vec<Struct>` payload and a ~200 B single struct, `serde_json` vs `sonic-rs`,
on this machine (aarch64). The numbers go in §8. If the win is not there, the
feature (§3.3) is not worth shipping and the façade is still worth having
(one seam, one place to change) — but we say so.

## 3. Phase 1 — `r2e_http::json` façade + R2E-owned `Json<T>`

### 3.1 `r2e_http::json`

```rust
pub fn to_vec<T: Serialize + ?Sized>(v: &T) -> Result<Vec<u8>, JsonError>;
pub fn to_string<T: Serialize + ?Sized>(v: &T) -> Result<String, JsonError>;
pub fn to_writer<W: io::Write, T: Serialize + ?Sized>(w: W, v: &T) -> Result<(), JsonError>;
pub fn from_slice<T: DeserializeOwned>(b: &[u8]) -> Result<T, JsonError>;
pub fn from_str<T: DeserializeOwned>(s: &str) -> Result<T, JsonError>;
pub struct JsonError { .. }   // Display + Error + Debug; `kind()` → Syntax | Data | Eof | Io
```

No `to_value` / `from_value` — those are `Value`-bound and stay `serde_json`.
`JsonError` wraps the backend's error and classifies it: `Data` (well-formed
JSON, wrong shape → 422 on the extractor) vs everything else (→ 400), which
is the distinction axum's `JsonRejection` makes and the one `map_error!`
users rely on. `From<serde_json::Error>` is implemented so the `Value` paths
(`serde_json::to_value`, `json!`) keep composing.

Re-exported as `r2e_core::json` (and `r2e::json`).

### 3.2 `r2e_http::Json<T>` (replaces the `axum::Json` re-export)

`pub struct Json<T>(pub T)` with `Deref`/`DerefMut`/`From<T>`, plus:

- `impl<S, T: DeserializeOwned> FromRequest<S> for Json<T>` — **bridge** to
  the backend's extraction contract (named bridge point, §5.3b table of the
  previous plan gains a row). Content-type check → `Bytes` → `json::from_slice`.
- `impl<S, T> OptionalFromRequest<S> for Json<T>` — `Option<Json<T>>` when
  the body is absent (no content-type), rejected otherwise as before.
- `impl<T: Serialize> IntoHttpResponse for Json<T>` + the backend bridge
  written out by hand (generic type — `impl_into_response!` is for
  non-generic ones).
- `JsonRejection` — `MissingContentType` (415), `BodyRead` (400/413 from the
  bytes rejection), `Syntax` (400), `Data` (422), `Eof` (400). Implements
  `IntoHttpResponse`, bridged with `impl_into_response!`.

Same public name, same field, same behaviour: the swap is source-compatible
for every consumer that writes `r2e_core::http::Json` / `r2e::http::Json`
(all of them — checked 2026-08-24). Anything that wrote `axum::Json` goes
through `axum_compat` and is on its own, as decided in §5.3d.

### 3.3 Backend selection

Cargo features on `r2e-http`, forwarded by `r2e-core` and `r2e`:

- default: `serde_json`.
- `json-sonic`: `sonic-rs`. Features are additive, so when both are present
  `sonic` wins (`#[cfg(feature = "json-sonic")]` takes precedence). No
  `simd-json`: it wants `&mut [u8]` input, which forces a copy of the request
  body in the extractor and makes the façade signature lie.

`serde_json` is never removed from `r2e-http`: `Value` / `json!` need it
(§1.3) and `JsonError: From<serde_json::Error>` needs it.

## 4. Phase 2 — Migrate the call sites, freeze the boundary

- `r2e-core`, `r2e-events` + backends, `r2e-security`, `r2e-openapi`,
  `r2e-oidc`, `r2e-openfga`, `r2e-utils`, `r2e-test`, `r2e-devservices`,
  `r2e-cli`: `serde_json::{to_vec,to_string,to_writer,from_slice,from_str}` →
  `r2e_core::json::…` (or `r2e_http::json` below core). `serde_json::Error`
  in public signatures (`WsError::Json`, `SseSerializeError`, `send_json`)
  → `JsonError`. **Breaking** for anyone matching on those.
- `r2e-macros`: `#[derive(Cacheable)]` and the `Cacheable for Json<T>` impl
  emit `#krate::json::…`.
- `scripts/check-source-boundary.sh` gains a third group, `serde_json`:
  pattern `serde_json::(to_vec|to_string|to_string_pretty|to_writer|from_slice|from_str|from_reader)\b`,
  baseline `scripts/boundaries/src-baseline-serde_json.txt`, by-design
  exclusion `r2e-http/src/json` (it IS the façade). `to_string_pretty` is
  allowed to stay in `r2e-openapi` (spec dump, not a hot path) via the
  baseline, not via the exclusion. No dep-allowlist for this group: the crate
  is legitimately everywhere for `Value`.

## 5. Phase 3 — Typed JWT claims in `r2e-security`

Why `Value` is there today: it is the *default* of an already-typed path
(`JwtClaimSet`, `JwtClaimsValidator::validate_as`,
`FromValidatedJwtClaims<S, C = Value>`), chosen because JWT claim sets are
open-ended and IdP-shaped. But `AuthenticatedUser` carries the whole tree and
every `RoleExtractor` walks it per request, while ~all reads hit a handful of
known fields.

Replace with a typed struct deserialized **directly** from the token:

```rust
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct StandardClaims {
    pub sub: String,
    pub email: Option<String>,
    pub exp: Option<u64>, pub iat: Option<u64>, pub nbf: Option<u64>,
    pub iss: Option<String>,
    pub aud: Option<Audience>,               // string | [string]
    pub preferred_username: Option<String>, pub name: Option<String>,
    pub scope: Option<String>,
    pub roles: Option<Vec<String>>,          // plain OIDC
    pub realm_access: Option<RealmAccess>,   // Keycloak
    pub resource_access: Option<HashMap<String, ClientAccess>>,
    #[serde(flatten)] pub extra: serde_json::Map<String, Value>,
}
```

- `AuthenticatedUser.claims: StandardClaims`; `Identity::claims()` returns
  `Option<&StandardClaims>` — **breaking**. A `get(&str) -> Option<&Value>`
  helper on `StandardClaims` keeps the "any custom claim" access, looking at
  `extra` only.
- `RoleExtractor::extract_roles(&self, claims: &StandardClaims)`: the
  Keycloak extractors become field reads instead of path walks.
- `JwtClaimSet` default `C = StandardClaims`; `Value` still implements the
  trait for the escape hatch.
- `jsonwebtoken` deserializes claims with `serde_json` internally; that is
  outside our boundary (one library call, counted once in the baseline).

## 6. Phase 4 — Docs

`llm.txt` (`Json`, `r2e::json`, `JsonError`, `StandardClaims`,
`json-sonic`), `docs/claude/prelude-features.md` (feature), `docs/claude/
subsystems.md` (security identity), the §5.3b bridge table of the previous
plan (new row for `Json<T>: FromRequest`). `CLAUDE.md` architecture entry for
`r2e-http`.

## 7. Definition of done

- `r2e_http::json` is the only place in `*/src/` that calls a codec for
  typed values; the serde_json baseline holds only `Value`-bound and
  by-design lines and is enforced in CI.
- `Json<T>` is R2E's type; `cargo test --workspace` green with the default
  backend and with `--features r2e-http/json-sonic`.
- Bench numbers recorded (§8), feature documented, breaking changes listed
  in the PR.

## 8. Execution log

- 2026-08-24 — Phase 0 bench, Apple Silicon (aarch64, NEON), `cargo bench -p r2e-http --bench json`:

  | shape | serde_json | sonic-rs | delta |
  |---|---|---|---|
  | `to_vec` one (~200 B) | 96 ns | 113 ns | sonic −15 % (slower) |
  | `to_vec` page (~3.4 KiB) | 3.21 µs | 2.28 µs | sonic **+30 %** |
  | `from_slice` one | 315 ns | 328 ns | ≈ |
  | `from_slice` page | 11.65 µs | 11.79 µs | ≈ |

  Verdict: on aarch64 the SIMD codec only pays on larger serialized responses;
  parsing is a wash. sonic-rs's ×2–3 parse figures are AVX2 (x86_64) numbers —
  to be re-measured on the deployment target before turning `json-sonic` on.
  The feature ships because it is cheap and the façade is the point; it is
  **off by default** and documented as "measure first".
- 2026-08-24 — Phase 1 landed: `r2e_http::json` façade (`to_vec`/`to_string`/
  `from_slice`/`from_str`, `JsonError` + `JsonErrorKind`, `BACKEND`), R2E-owned
  `Json<T>` (+ `JsonRejection`, `OptionalFromRequest`), feature `json-sonic`.
  `to_writer` dropped from the façade (backends disagree on the writer bound,
  no caller). Workspace compiles unchanged against the new type.
- 2026-08-24 — Phase 2 landed: every typed codec call in `*/src/` goes through
  the façade (`r2e-core` error/ws/sse/`Cacheable`, `#[derive(Cacheable)]`
  emitting `#krate::json::…`, all four event backends + `pending.rs`/`state.rs`,
  `r2e-oidc`, `r2e-openfga`, `r2e-test`, `r2e-security` JWKS parse). Boundary
  group `serde_json` enforced; baseline holds 5 lines, all reviewed:
  `r2e-cli` (`cargo metadata` → `Value`, no `r2e-core` dep), `r2e-devservices`
  (`Value`, no `r2e-core` dep), `r2e-openapi` (`to_string_pretty` spec dump),
  `r2e-openfga/macros` (build-time), `r2e-test` (`json_value` → `Value`).
  Breaking: `WsError::Json(JsonError)`, `WsBroadcaster::send_json` /
  `send_json_from` → `Result<_, JsonError>`, `WsTestError::Json(JsonError)`.
  `SseSerializeError` unchanged (already boxed). New tests:
  `r2e-core/tests/http/json.rs` (status policy, both backends).
- 2026-08-24 — Phase 3 landed: `StandardClaims` (+ `Audience`, `RealmAccess`,
  `ClientAccess`) in `r2e-core/src/decorators/claims.rs` — in `r2e-core`
  because `Identity::claims()` is declared there; re-exported by
  `r2e-security` and the prelude. Every claims-carrying signature retyped
  (`Identity::claims`, `GuardContext::identity_claims`,
  `GrpcGuardContext::identity_claims`, `RoleExtractor::extract_roles`,
  `AuthenticatedUser.claims`/`from_claims`/`from_claims_with`,
  `build_authenticated_user`, `IdentityBuilder::build`,
  `FromValidatedJwtClaims<S, C = StandardClaims>`,
  `JwtClaimsValidator::validate`, `JwtValidator::validate_claims`,
  `extract_jwt_claims`, `impl_claims_identity_extractor!` default) —
  **breaking**. Decisions beyond §5: `PartialEq` derived; `sub` defaults to
  `""` and `subject()` maps it to `None` (keeps the precise "no sub"
  rejection); `AuthenticatedUser` keeps owned `sub`/`email`/`roles`;
  optionals `skip_serializing_if` so re-serializing reproduces the payload;
  `get()` reads `extra` only. Escape hatch kept: `JwtClaimSet for Value`,
  `validate_as::<C>`, `extract_jwt_claims_as`. Follow-up: gRPC
  `GrpcIdentityExtractor` / `JwtClaimsValidatorLike` still return `Value`.
- 2026-08-24 — Phase 4 docs: `llm.txt` (JSON codec paragraph, `Json<T>`
  promise, `StandardClaims` section, custom-identity examples),
  `CLAUDE.md`, `docs/claude/{prelude-features,subsystems,guards-interceptors}.md`,
  bridge table row in the previous plan. Boundary check green on all three
  groups; workspace compiles on both backends.
