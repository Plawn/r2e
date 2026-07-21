# r2e-openfga — Test Development Plan

## Coverage Gaps (llvm-cov 2026-07-21)

- **Line coverage**: 55.4% (835/1508)
- **Function coverage**: 59.5% (131/220)

| File | Lines | Covered | Coverage | Uncovered |
|------|------:|--------:|---------:|----------:|
| `src/plugin.rs` | 377 | 53 | 14.1% | 324 |
| `src/backend.rs` | 237 | 77 | 32.5% | 160 |
| `src/config.rs` | 62 | 0 | 0.0% | 62 |
| `src/typed.rs` | 118 | 64 | 54.2% | 54 |
| `src/guard.rs` | 185 | 157 | 84.9% | 28 |
| `src/error.rs` | 21 | 4 | 19.0% | 17 |
| `src/cache.rs` | 109 | 93 | 85.3% | 16 |
| `src/model_convert.rs` | 291 | 284 | 97.6% | 7 |
| `src/registry.rs` | 55 | 50 | 90.9% | 5 |

### `src/plugin.rs` — 324 uncovered lines (14.1%)

Almost the entire plugin is uncovered. The existing `plugin_provides_fga_client` test only checks that the bean is provided; the boot sequence, config validation, and store/model resolution are never exercised.

| Missing test | Uncovered code path |
|---|---|
| `validate_config_requires_endpoint` | `validate_config()` panics when `endpoint` is None/empty (L222-228) |
| `validate_config_requires_store_or_store_id` | `validate_config()` panics when both `store`/`store_id` are None (L229-236) |
| `validate_config_rejects_model_id_in_apply_mode` | `validate_config()` panics when `apply_model=true` + `model_id` is set (L238-244) |
| `validate_config_defaults` | Default timeout values (connect=10, request=5) from config (L245-253) |
| `boot_connect_failure` | `boot()` → `connect_client()` failure returns actionable error (L326-337) |
| `boot_resolve_store_by_id` | `resolve_store()` with `StoreSelector::Id` — GetStore RPC (L364-381) |
| `boot_resolve_store_by_name_single` | `resolve_store()` with `StoreSelector::Name`, 1 match (L384-385) |
| `boot_resolve_store_by_name_creates_in_apply` | `resolve_store()` with `Name`, 0 matches, `apply_model=true` → CreateStore (L386-403) |
| `boot_resolve_store_by_name_fails_in_verify` | 0 matches + `apply_model=false` → error (L404-408) |
| `boot_resolve_store_by_name_multiple_apply` | Multiple matches + apply → uses oldest (L409-427) |
| `boot_resolve_store_by_name_multiple_verify` | Multiple matches + verify → error (L428-432) |
| `boot_resolve_model_apply_unchanged` | `resolve_model()` apply mode, model unchanged → reuse (L518-523) |
| `boot_resolve_model_apply_writes_new` | Model differs → WriteAuthorizationModel (L525-545) |
| `boot_resolve_model_verify_matches` | Verify mode, model matches → Ok (L505-513) |
| `boot_resolve_model_verify_pinned` | Verify mode with `model_id` → ReadAuthorizationModel by id (L476-501) |
| `boot_resolve_model_verify_mismatch` | Verify mode, model differs → BootError with diff (L575-591) |
| `lazy_backend_not_ready` | `LazyBackend` before boot → `NotReady` error (L604+) |
| `disabled_plugin_skips_boot` | `enabled: false` → no post_construct, checks fail closed (L188-195) |
| `invalid_model_json_panics_at_install` | Bad model JSON → install-time panic (L200-202) |

**Blocker**: All boot paths require a real or mocked gRPC `OpenFgaServiceClient<Channel>`. Either:
- Use `r2e-devservices` `DevOpenFga` container (integration test)
- Build a tonic mock service (unit test) — a `tower::Service` that handles `Check`/`Write`/`ListStores`/`ReadAuthorizationModel`

### `src/backend.rs` — 160 uncovered lines (32.5%)

The entire `GrpcBackend` impl (check/write_tuple/delete_tuple, connect, request_with_token) and `connect_client` are uncovered. Only `MockBackend` is tested.

| Missing test | Uncovered code path |
|---|---|
| `grpc_connect_sets_timeouts` | `connect_client()` applies connect_timeout + request_timeout (L175-187) |
| `request_with_token_adds_bearer` | `request_with_token(Some("tok"), msg)` injects `Authorization: Bearer tok` header (L192-208) |
| `request_with_token_none_skips_header` | `request_with_token(None, msg)` adds no metadata (L196) |
| `request_with_token_invalid_header_errors` | Invalid token chars → `InvalidConfig` error (L200-204) |
| `grpc_check_builds_correct_request` | `GrpcBackend::check()` populates `CheckRequest` with store_id, model_id, tuple_key (L211-237) |
| `grpc_write_tuple_builds_correct_request` | `write_tuple()` sends correct `WriteRequest` with writes (L240-268) |
| `grpc_delete_tuple_builds_correct_request` | `delete_tuple()` sends correct `WriteRequest` with deletes (L271-299) |
| `config_validate_empty_endpoint` | `OpenFgaConfig::validate()` rejects empty endpoint (L119-123) |
| `config_validate_empty_store_id` | `OpenFgaConfig::validate()` rejects empty store_id (L124-128) |

**Blocker**: Same as plugin — needs gRPC mock or live server.

### `src/config.rs` — 62 uncovered lines (0%)

Entirely untested. Builder API and serde defaults.

| Missing test | Uncovered code path |
|---|---|
| `config_new_defaults` | `OpenFgaConfig::new()` sets defaults (connect_timeout=10, request_timeout=5, cache=true, cache_ttl=60) |
| `config_with_model_id` | `.with_model_id("m")` sets model_id |
| `config_with_api_token` | `.with_api_token("t")` sets api_token |
| `config_with_connect_timeout` | `.with_connect_timeout(30)` overrides |
| `config_with_request_timeout` | `.with_request_timeout(15)` overrides |
| `config_with_cache` | `.with_cache(false, 120)` sets both fields |
| `config_without_cache` | `.without_cache()` sets cache_enabled=false |
| `config_deserialize_defaults` | Serde defaults applied when fields absent in YAML |
| `config_validate_ok` | Non-empty endpoint + store_id → Ok |
| `config_validate_empty_endpoint` | Empty endpoint → Err |
| `config_validate_empty_store_id` | Empty store_id → Err |

**No blocker** — pure struct, no I/O.

### `src/typed.rs` — 54 uncovered lines (54.2%)

FgaObject/FgaRel/FgaUserset/FgaWildcard manual trait impls (Debug, Clone, PartialEq, Eq, Hash, Display) and the `FgaUserset`/`FgaWildcard` API surface.

| Missing test | Uncovered code path |
|---|---|
| `fga_object_debug_format` | `Debug` impl for `FgaObject` (L53-56) |
| `fga_object_clone_eq_hash` | `Clone`, `PartialEq`, `Hash` impls (L58-76) |
| `fga_rel_of_builds_userset` | `FgaRel::of(obj)` → `FgaUserset` with `type:id#relation` (L147-152) |
| `fga_userset_debug_clone_eq_hash` | `FgaUserset` trait impls (L168-191) |
| `fga_userset_as_str` | `FgaUserset::as_str()` returns full `type:id#relation` (L193-198) |
| `fga_userset_display` | `Display` impl (L200-204) |
| `fga_wildcard_debug_clone_eq` | `FgaWildcard` trait impls (L210-226) |
| `fga_wildcard_new_display` | `FgaWildcard::new()` + Display → `type:*` (L228-231) |

**No blocker** — pure types, no I/O.

### `src/error.rs` — 17 uncovered lines (19%)

Display impl for most variants and the `From<tonic::Status>` conversion.

| Missing test | Uncovered code path |
|---|---|
| `display_all_variants` | Display formatting for each `OpenFgaError` variant (L32-48) |
| `from_tonic_deadline_exceeded` | `tonic::Status::deadline_exceeded` → `Timeout` (L62) |
| `from_tonic_other_status` | Other tonic status → `ServerError` (L63) |
| `from_transport_error` | `tonic::transport::Error` → `ConnectionFailed` (L54-56) |

**No blocker** — pure error types.

### `src/cache.rs` — 16 uncovered lines (85.3%)

`DecisionCache::new()` constructor defaults and `invalidate_user()`.

| Missing test | Uncovered code path |
|---|---|
| `cache_new_defaults` | `DecisionCache::new(60)` sets default max_entries=10_000 (L28-56) |
| `cache_invalidate_user` | `invalidate_user("user:alice")` removes all entries for that user (L131-138) |
| `cache_invalidate_object` | `invalidate_object("doc:1")` removes matching entries (L120-128) |

**No blocker** — pure in-memory.

---

## Current State

- **~70 tests** (cache: 3, registry: 3, backend: 3, client: 7, guard: 3, model_convert: 2, plugin: 1, typed: 4, model/parser: 20+, model/validate: 14)
- **Line coverage**: 55.4%
- **Key gap**: plugin boot sequence (324 uncov), GrpcBackend runtime (160 uncov), config (62 uncov, 0%)

---

## Phase 1: Config + Error + Typed (Quick Wins, No I/O)

**Target**: +140 lines covered (~9% bump)

### `tests/config.rs` (new)

| Test | Uncovered lines |
|------|-----------------|
| `config_new_defaults` | L67-78 |
| `config_with_model_id` | L81-84 |
| `config_with_api_token` | L87-90 |
| `config_with_connect_timeout` | L93-96 |
| `config_with_request_timeout` | L99-102 |
| `config_with_cache` | L105-109 |
| `config_without_cache` | L112-115 |
| `config_validate_ok` | L118-130 |
| `config_validate_empty_endpoint` | L119-123 |
| `config_validate_empty_store_id` | L124-128 |
| `config_deserialize_yaml_defaults` | L6-17 (serde defaults) |

### `tests/error.rs` (new)

| Test | Uncovered lines |
|------|-----------------|
| `display_all_variants` | L32-48 |
| `from_transport_error` | L54-56 |
| `from_tonic_deadline_exceeded` | L62 |
| `from_tonic_other_status` | L63 |

### `tests/typed.rs` (extend)

| Test | Uncovered lines |
|------|-----------------|
| `fga_object_debug_clone_eq_hash` | L53-76 |
| `fga_rel_of_builds_userset` | L147-152 |
| `fga_userset_as_str_display` | L193-204 |
| `fga_wildcard_new_debug_display` | L210-231 |

---

## Phase 2: Cache Extended

**Target**: +16 lines covered

### `tests/cache.rs` (extend)

| Test | Uncovered lines |
|------|-----------------|
| `cache_new_default_capacity` | L28-56 |
| `cache_invalidate_user` | L131-138 |
| `cache_invalidate_object` | L120-128 |

---

## Phase 3: Plugin Config Validation (No gRPC)

**Target**: +30 lines covered

The `validate_config()` function is pure — it panics on bad input without I/O.

### `tests/plugin.rs` (extend)

| Test | Uncovered lines |
|------|-----------------|
| `validate_config_requires_endpoint` | L222-228 |
| `validate_config_requires_store_or_store_id` | L229-236 |
| `validate_config_rejects_model_id_in_apply_mode` | L238-244 |
| `validate_config_stores_defaults` | L245-253 |
| `validate_config_accepts_store_id_only` | L230 |
| `validate_config_accepts_store_name_only` | L231 |
| `invalid_model_json_panics_at_install` | L200-202 |

**Blocker**: `validate_config` is private. Needs `#[doc(hidden)] pub` or a test helper, or test via `OpenFga::model().install()` with crafted config.

---

## Phase 4: Backend request_with_token (No gRPC Connection)

**Target**: +16 lines covered

`request_with_token` is `pub(crate)` and pure — builds a `tonic::Request` with metadata.

### `tests/backend.rs` (extend)

| Test | Uncovered lines |
|------|-----------------|
| `request_with_token_adds_bearer` | L192-208 |
| `request_with_token_none_skips_header` | L196 |
| `request_with_token_invalid_header_errors` | L200-204 |

---

## Phase 5: Plugin Boot Sequence (Requires gRPC Mock or DevOpenFga)

**Target**: +~290 lines covered (~19% bump)

Covers `boot()`, `resolve_store()`, `stores_by_name()`, `resolve_model()`, `latest_model()`, `verify()`, `LazyBackend`, `GrpcBackend` impls.

### Option A: tonic mock service (unit tests, fast)

Build a `MockOpenFgaService` implementing `OpenFgaService` (the openfga-rs trait), start a tonic server on a random port, connect `GrpcBackend` / the plugin to it.

### Option B: DevOpenFga container (integration tests, thorough)

Use `r2e-devservices` `DevOpenFga` — real server, tests the actual gRPC wire format.

| Test | Uncovered code path |
|------|---------------------|
| `boot_resolve_store_by_id` | resolve_store StoreSelector::Id (L364-381) |
| `boot_resolve_store_by_name_single` | resolve_store Name, 1 match (L384-385) |
| `boot_create_store_apply_mode` | Name, 0 matches, apply=true → CreateStore (L386-403) |
| `boot_resolve_store_verify_missing` | Name, 0 matches, apply=false → error (L404-408) |
| `boot_resolve_model_apply_unchanged` | Same model → reuse (L518-523) |
| `boot_resolve_model_apply_writes` | Different model → WriteAuthorizationModel (L525-545) |
| `boot_resolve_model_verify_ok` | verify mode, model matches → Ok (L505-513) |
| `boot_resolve_model_verify_mismatch` | verify mode, model differs → BootError (L575-591) |
| `boot_resolve_model_verify_pinned` | verify + model_id → ReadAuthorizationModel by id (L476-501) |
| `disabled_plugin_skips_boot` | enabled=false, no connection attempt (L188-195) |
| `lazy_backend_not_ready_before_boot` | LazyBackend.check/write before boot → NotReady (L604+) |
| `grpc_check_roundtrip` | GrpcBackend::check() → CheckRequest → response (L211-237) |
| `grpc_write_tuple_roundtrip` | GrpcBackend::write_tuple() (L240-268) |
| `grpc_delete_tuple_roundtrip` | GrpcBackend::delete_tuple() (L271-299) |
| `grpc_connect_failure` | connect_client to unreachable → ConnectionFailed (L175-187) |

---

## Estimated Effort

| Phase | Tests | Lines recovered | Effort | Blocker |
|-------|------:|---------:|--------|---------|
| Phase 1 | 19 | ~140 | 1.5h | None |
| Phase 2 | 3 | ~16 | 30m | None |
| Phase 3 | 7 | ~30 | 1h | `validate_config` visibility |
| Phase 4 | 3 | ~16 | 30m | None |
| Phase 5 | 15 | ~290 | 4-6h | gRPC mock or DevOpenFga |
| **Total** | **47** | **~492** | **~8-10h** | |

**Projected coverage after all phases**: ~88% (1327/1508)
