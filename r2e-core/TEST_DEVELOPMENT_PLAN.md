# r2e-core — Test Development Plan

## Coverage Snapshot (llvm-cov — 2026-07-21)

- **Line coverage**: 74.9% (3860 / 5156)
- **Function coverage**: 75.3% (2018 / 2680)
- **Uncovered lines**: 1296
- **Existing tests**: ~574

### Update 2026-07-21 (evening) — Bean/DI wave done

29 tests added (`tests/lazy.rs`, `tests/beans.rs`, `tests/builder_hlist.rs`). New `cargo llvm-cov --workspace` numbers:

- `src/lazy.rs`: 39.8% → **94.1%** (remainder: `lazy-fallback-runtime` cfg arms, measured only under that feature)
- `src/beans.rs`: 69.2% → **86.4%** (remainder: dev-reload-only paths — `try_get_eager`, `ReusePlan` carryover — exercised by `tests/dev_reload_partial.rs` under `--features dev-reload`)
- `src/builder/nostate.rs`: 80.7% → **94.7%**
- r2e-core total: 74.9% → **80.2%**; workspace total: 68.1% → **69.2%**

---

## Uncovered Areas by File (sorted by gap size)

### 1. `src/builder/nostate.rs` — ~704 uncovered lines

Largest single gap. Most uncovered code is in the `AppBuilder` pre-state methods.

| Uncovered range | What it covers |
|---|---|
| L122–148 | `provide_with_post_construct`, `provide_with_pre_destroy` |
| L150–260 | `register::<T: Registrable>()` — controller/bean registration |
| L262–400 | `with_plugin()` (deprecated), `plugin()`, `add_deferred()` |
| L400–600 | `build_state()` — bean graph resolution, fingerprint, overlay |
| L600–826 | dev-reload `build_state` path, reuse plan, state diff |

**Tests needed:**

| Test | Description |
|---|---|
| `provide_with_post_construct_runs_hook` | ✅ DONE (`tests/builder_hlist.rs::provide_with_post_construct_runs_hook_during_build_state`) |
| `provide_with_pre_destroy_registers_disposer` | ✅ DONE (`tests/builder_hlist.rs::provide_with_pre_destroy_defers_hook_to_shutdown` + e2e `tests/builder_prepared.rs::provide_with_pre_destroy_runs_disposer_on_graceful_shutdown`; order covered at registry level in `tests/beans.rs`) |
| `register_missing_dep_panics` | N/A — compile-time check (`AllSatisfied`), covered by r2e-compile-tests |
| `build_state_resolves_graph` | ✅ already covered (`tests/builder_hlist.rs::build_state_materializes_hlist_from_provisions`) |
| `build_state_overlay_merges` | ✅ DONE (`tests/beans.rs::bean_context_snapshot_does_not_see_later_beans`) |
| `build_state_lazy_slots_deferred` | ✅ DONE (`tests/beans.rs::lazy_bean_constructed_on_first_get_only`) |
| `build_state_fingerprint_computed` | ✅ DONE (`tests/beans.rs::compute_fingerprint_*`, 3 tests, feature `dev-reload`) |
| `build_state_dev_reload_reuse` | ✅ already covered (`tests/dev_reload_partial.rs`) |
| `deferred_action_added_to_prepared` | Open — covered indirectly by plugin tests; no direct test |

Also added 2026-07-21: `with_default_async_bean_builds`, `with_default_producer_builds`, `with_bean_factory_reads_config` (`tests/builder_hlist.rs`).

### 2. `src/beans.rs` — ~470 uncovered lines

Bean registry internals and context overlay.

| Uncovered range | What it covers |
|---|---|
| L188–336 | `BeanContext` struct (base/overlay/lazy_slots/disposers), `Clone`, `Debug`, `empty()` |
| L448–656 | `BeanRegistry` type definitions: `Factory`, `PostConstructFn`, `DisposerBuilder`, `ScheduledSourceHook`, `EventSubscriberHook`, `DecoFillHook`, `LazyBeanRegistration`, `BeanRegistration`, `ReusePlan`, `RegMeta` trait |
| L1213–1313 | `compute_fingerprint()` (dev-reload), `resolve()` |
| L1491–1554 | Lazy bean validation and slot construction in `resolve_reusing` |

**Tests needed:**

| Test | Description |
|---|---|
| `bean_context_clone_shares_arc` | ✅ DONE (`bean_context_clone_shares_lazy_slots`, `bean_context_clone_does_not_carry_disposers`) |
| `bean_context_empty_has_no_beans` | ✅ DONE (`bean_context_empty_has_no_beans_and_debug_shows_counts`) |
| `bean_context_overlay_shadows_base` | ✅ DONE (`bean_context_snapshot_does_not_see_later_beans` — overlay insert + snapshot isolation) |
| `bean_context_lazy_slot_resolves_on_get` | ✅ DONE (`lazy_bean_constructed_on_first_get_only` + `lazy_to_lazy_dependency_resolves_on_first_get` + `lazy_async_bean_resolves_on_first_get`) |
| `bean_registry_factory_produces_bean` | ✅ already covered (`resolve_simple_graph`) |
| `bean_registry_post_construct_fires` | ✅ already covered (`post_construct_is_called`) |
| `bean_registry_disposer_fires_on_shutdown` | ✅ already covered (`pre_destroy_disposers_run_in_reverse_registration_order`) |
| `compute_fingerprint_stable` | ✅ DONE (`compute_fingerprint_stable_for_same_graph`, feature `dev-reload`) |
| `compute_fingerprint_changes_on_diff` | ✅ DONE (`compute_fingerprint_changes_when_graph_differs`, `compute_fingerprint_changes_on_config_edit`) |
| `resolve_reusing_skips_unchanged` | ✅ already covered (`tests/dev_reload_partial.rs::partial_rebuild_reuses_unchanged_beans_across_cycles`) |

Also added 2026-07-21 (lazy graph paths): `lazy_bean_missing_dependency_errors_at_resolve`, `lazy_bean_registered_twice_is_duplicate`, `lazy_bean_conflicting_with_provided_is_duplicate`, `lazy_bean_conflicting_with_eager_registration_is_duplicate`, `lazy_default_superseded_by_later_registration`, `lazy_bean_required_config_key_missing_fails_at_resolve` / `_present_resolves`, `lazy_bean_optional_config_key_absent_is_fine`.

### 3. `src/ws.rs` — ~306 uncovered lines

WebSocket abstractions — almost entirely untested.

| Uncovered range | What it covers |
|---|---|
| L40–160 | `WsStream`: `send_text`, `send_json`, `send_binary`, `next`, `next_text`, `next_json`, `on_each` |
| L160–220 | `WsHandler` trait, `run_ws_handler` |
| L220–280 | `WsBroadcaster`: `send_json_from`, `send_from` |
| L280–346 | `WsBroadcastReceiver`, `WsRooms` |

**Tests needed:**

| Test | Description |
|---|---|
| `ws_stream_send_text` | `WsStream::send_text` sends text frame |
| `ws_stream_send_json` | `WsStream::send_json` serializes and sends JSON frame |
| `ws_stream_send_binary` | `WsStream::send_binary` sends binary frame |
| `ws_stream_next_text` | `WsStream::next_text` reads next text message |
| `ws_stream_next_json` | `WsStream::next_json` deserializes JSON from next message |
| `ws_broadcaster_fanout` | `WsBroadcaster::send_from` sends to all receivers |
| `ws_broadcaster_json_fanout` | `WsBroadcaster::send_json_from` serializes and fans out |
| `ws_rooms_join_leave` | `WsRooms` join/leave track membership |
| `ws_rooms_broadcast_to_room` | Broadcast targets only members of the specified room |
| `ws_handler_trait_dispatches` | `run_ws_handler` invokes `WsHandler::on_message` |

### 4. `src/multipart.rs` — ~290 uncovered lines

Multipart extraction — entirely untested.

| Uncovered range | What it covers |
|---|---|
| L37–97 | `MultipartError` variants (Display, IntoResponse) |
| L99–273 | `UploadedFile`, `MultipartFields` (collect_from, collect_from_with_limits, take_text, take_text_opt, take_file, take_file_opt, take_files, take_bytes) |
| L276–342 | `FromMultipart` trait, `TypedMultipart` extractor (FromRequest impl) |

**Tests needed:**

| Test | Description |
|---|---|
| `multipart_error_missing_field_400` | `MissingField` → 400 Bad Request |
| `multipart_error_parse_error_400` | `ParseError` → 400 Bad Request |
| `multipart_error_field_too_large_413` | `FieldTooLarge` → 413 Payload Too Large |
| `multipart_error_payload_too_large_413` | `PayloadTooLarge` → 413 Payload Too Large |
| `multipart_fields_collect_text` | Text fields collected into `text` map |
| `multipart_fields_collect_file` | File fields (with filename) collected into `files` map |
| `multipart_fields_per_field_limit` | Exceeding per-field limit returns `FieldTooLarge` |
| `multipart_fields_total_limit` | Exceeding total limit returns `PayloadTooLarge` |
| `multipart_take_text_returns_first` | `take_text` returns and removes first value |
| `multipart_take_text_missing_errors` | `take_text` on missing field returns `MissingField` |
| `multipart_take_file_returns_first` | `take_file` returns and removes first file |
| `multipart_take_bytes_file_first` | `take_bytes` prefers file data over text |
| `uploaded_file_len_is_empty` | `UploadedFile::len()` / `is_empty()` correct |

### 5. `src/lazy.rs` — ~252 uncovered lines

Lazy bean resolution — critical control-plane path untested.

| Uncovered range | What it covers |
|---|---|
| L24–50 | `ResolutionGuard` — circular dependency detection (thread-local stack) |
| L52–115 | `LazySlot<T>` — `new`, `get_or_init`, `LazyResolve` impl |
| L117–211 | `resolve_lazy_factory` — control-plane path (sharded worker), multi-thread `block_in_place`, fallback runtime |
| L237–316 | `Lazy<T>` (deprecated) — `new`, `get`, `Clone` |

**Tests needed:**

| Test | Description |
|---|---|
| `resolution_guard_detects_cycle` | ✅ DONE (`circular_lazy_dependency_panics_with_cycle_trace` — A→B→A trace) |
| `resolution_guard_cleans_up_on_drop` | ✅ DONE (implicit: successful lazy resolutions push+pop; repeat gets stay clean) |
| `lazy_slot_resolves_once` | ✅ DONE (`tests/beans.rs::lazy_bean_constructed_on_first_get_only`) |
| `lazy_slot_cached_after_first` | ✅ DONE (same test — second `get` does not re-run factory) |
| `resolve_lazy_factory_multithread` | ✅ DONE (`resolve_lazy_factory_on_multi_thread_runtime`) |
| `resolve_lazy_factory_control_plane` | ✅ DONE (`resolve_lazy_factory_uses_control_plane_when_registered` + `_panic_resurfaces`) |
| `resolve_lazy_factory_no_runtime_panics` | ✅ DONE (`resolve_lazy_factory_without_runtime_panics`, `_current_thread_runtime_panics`; fallback-feature variants also added) |
| `lazy_wrapper_get_resolves` | ✅ already covered (`lazy_resolves_once`) |
| `lazy_wrapper_clone_shares_cell` | ✅ DONE (`lazy_clone_shares_cell`) |

Note (2026-07-21): the `lazy-fallback-runtime` branches originally used `Runtime::block_on`, which panics from within async execution ("Cannot start a runtime from within a runtime") — i.e. the feature's main advertised case. FIXED the same day: both fallback branches now route through the shared `resolve_on` spawn+channel helper (same mechanism as the control-plane path); `resolve_lazy_factory_falls_back_on_current_thread_runtime` proves the async-context case and asserts the factory ran off-thread.

### 6. `src/error.rs` — ~224 uncovered lines

HttpError constructors and conversions — partially tested but many gaps.

| Uncovered range | What it covers |
|---|---|
| L29–84 | `HttpError` enum definition, `Clone` impl |
| L86–183 | `from_status`, convenience constructors (`internal`, `not_found`, etc.), `status()`, `message()`, `context()` |
| L187–253 | `IntoResponse` impl, `Display`, `Debug`, `Error::source()` |

**Tests needed:**

| Test | Description |
|---|---|
| `http_error_clone_preserves_variant` | `Clone` on each variant produces equal error |
| `http_error_from_status_known` | `from_status(404, msg)` → `NotFound` variant |
| `http_error_from_status_unknown` | `from_status(418, msg)` → `Custom` variant |
| `http_error_context_prefixes_message` | `.context("ctx")` prepends to message |
| `http_error_context_noop_on_custom` | `.context()` on `Custom`/`Validation` is identity |
| `http_error_message_returns_cow` | `.message()` returns inner `Cow` for simple variants |
| `http_error_with_source_preserves_chain` | `WithSource` variant `.source()` returns original error |
| `http_error_into_response_json` | `IntoResponse` produces `{"error": "..."}` body |
| `http_error_validation_into_response` | `Validation` variant → 400 with `details` array |
| `http_error_display_format` | `Display` format matches `"Variant: message"` |
| `http_error_from_io_error` | `From<std::io::Error>` → `Internal` |

### 7. `src/config/value.rs` — ~153 uncovered lines

ConfigValue conversions.

| Uncovered range | What it covers |
|---|---|
| L40–90 | `ConfigValue` `Hash` impl, `from_yaml` |
| L90–193 | `From` impls (u16, f64, bool, etc.), `FromConfigValue` for String |

**Tests needed:**

| Test | Description |
|---|---|
| `config_value_hash_consistency` | Equal `ConfigValue`s produce same hash |
| `config_value_from_yaml_string` | YAML string → `ConfigValue::String` |
| `config_value_from_yaml_number` | YAML number → `ConfigValue::Number` |
| `config_value_from_yaml_bool` | YAML bool → `ConfigValue::Bool` |
| `config_value_from_yaml_nested` | YAML mapping → nested `ConfigValue::Map` |
| `config_value_from_u16` | `From<u16>` conversion |
| `config_value_from_f64` | `From<f64>` conversion |
| `from_config_value_string` | `FromConfigValue` for `String` extracts value |

### 8. `src/config/validation.rs` — ~40 uncovered lines

Config validation error collection.

| Uncovered range | What it covers |
|---|---|
| L19–38 | `MissingKeyError` struct, `Display` impl |
| L172–212 | `collect_config_errors` — `Validation`, `NotFound`, `Deserialize`, `Load` branches |

**Tests needed:**

| Test | Description |
|---|---|
| `missing_key_error_display_format` | Display shows source, key, type |
| `missing_key_error_with_env_hint` | Display includes env var hint when present |
| `collect_config_errors_validation` | `ConfigError::Validation` → `MissingKeyError` with message |
| `collect_config_errors_not_found` | `ConfigError::NotFound` → `MissingKeyError` with "unknown" type |
| `collect_config_errors_deserialize` | `ConfigError::Deserialize` → `MissingKeyError` with message |
| `collect_config_errors_load` | `ConfigError::Load` → `MissingKeyError` with "loadable" type |

### 9. `src/secure_headers.rs` — ~85 uncovered lines

Secure headers builder methods.

| Uncovered range | What it covers |
|---|---|
| L70–99 | `SecureHeadersBuilder` struct, `new()` defaults |
| L99–154 | Builder methods: `content_type_options`, `frame_options`, `no_frame_options`, `hsts`, `hsts_max_age`, `hsts_include_subdomains`, `xss_protection`, `referrer_policy`, `content_security_policy` |

**Tests needed:**

| Test | Description |
|---|---|
| `secure_headers_defaults` | Default builder produces all standard headers |
| `secure_headers_disable_hsts` | `.hsts(false)` omits HSTS header |
| `secure_headers_custom_frame_options` | `.frame_options("SAMEORIGIN")` changes value |
| `secure_headers_no_frame_options` | `.no_frame_options()` omits X-Frame-Options |
| `secure_headers_custom_csp` | `.content_security_policy("...")` sets CSP |
| `secure_headers_custom_referrer` | `.referrer_policy("no-referrer")` overrides |

### 10. `src/builder/prepared.rs` — ~85 uncovered lines

Prepared builder (post-build_state) — shutdown composition, QUIC endpoint, startup hooks.

| Uncovered range | What it covers |
|---|---|
| L131–148 | dev-reload + sharding warning path |
| L277–303 | Startup hooks execution, dev-reload lifecycle mark |
| L307–362 | QUIC endpoint binding, shutdown future composition |

**Tests needed:**

| Test | Description |
|---|---|
| `startup_hooks_run_in_order` | Startup hooks fire sequentially before serve |
| `startup_hook_error_aborts` | Startup hook returning `Err` aborts launch |
| `shutdown_future_composes_drain_and_plugins` | Shutdown future awaits drain hooks then plugin hooks |

---

## Priority Order

1. **multipart.rs** — entirely untested, user-facing extraction, security-relevant (limits)
2. **error.rs** — partially tested but many gaps in core error handling
3. **lazy.rs** — critical DI path (circular detection, control-plane resolution)
4. **ws.rs** — entirely untested, user-facing WebSocket API
5. **builder/nostate.rs** — large gap but mostly internal; test via integration tests
6. **beans.rs** — internal registry; test overlay/lazy/fingerprint paths
7. **config/value.rs** — conversion correctness
8. **config/validation.rs** — small gap, straightforward
9. **secure_headers.rs** — builder pattern, straightforward
10. **builder/prepared.rs** — shutdown paths, harder to test in isolation

## Estimated Effort

| Area | New tests | Effort |
|---|---|---|
| multipart.rs | 13 | 3h |
| error.rs | 11 | 2h |
| lazy.rs | 9 | 3h |
| ws.rs | 10 | 4h |
| builder/nostate.rs | 9 | 4h |
| beans.rs | 10 | 3h |
| config/value.rs | 8 | 1.5h |
| config/validation.rs | 6 | 1h |
| secure_headers.rs | 6 | 1h |
| builder/prepared.rs | 3 | 2h |
| **Total** | **85** | **~24.5h** |
