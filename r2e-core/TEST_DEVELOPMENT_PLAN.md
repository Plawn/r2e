# r2e-core — Test Development Plan

## Coverage Snapshot (llvm-cov — 2026-07-21)

- **Line coverage**: 74.9% (3860 / 5156)
- **Function coverage**: 75.3% (2018 / 2680)
- **Uncovered lines**: 1296
- **Existing tests**: ~574

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
| `provide_with_post_construct_runs_hook` | Post-construct hook fires during `build_state()` |
| `provide_with_pre_destroy_registers_disposer` | Pre-destroy hook stored for shutdown |
| `register_missing_dep_panics` | `register::<T>()` with unsatisfied dep → compile/panic diagnostic |
| `build_state_resolves_graph` | Full bean graph resolution produces correct HList state |
| `build_state_overlay_merges` | Overlay beans override base beans |
| `build_state_lazy_slots_deferred` | `#[bean(lazy)]` slots are not resolved eagerly |
| `build_state_fingerprint_computed` | `compute_fingerprint` produces stable hash for same graph |
| `build_state_dev_reload_reuse` | dev-reload path reuses unchanged beans (feature `dev-reload`) |
| `deferred_action_added_to_prepared` | `add_deferred()` actions appear in prepared builder |

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
| `bean_context_clone_shares_arc` | `BeanContext::clone()` shares underlying data |
| `bean_context_empty_has_no_beans` | `BeanContext::empty()` has zero beans |
| `bean_context_overlay_shadows_base` | Overlay bean returned instead of base for same type |
| `bean_context_lazy_slot_resolves_on_get` | Lazy slot factory runs on first `get::<T>()` |
| `bean_registry_factory_produces_bean` | `Factory` closure produces the registered bean |
| `bean_registry_post_construct_fires` | `PostConstructFn` runs after bean creation |
| `bean_registry_disposer_fires_on_shutdown` | `DisposerBuilder` produces disposer that runs at shutdown |
| `compute_fingerprint_stable` | Same registrations → same fingerprint |
| `compute_fingerprint_changes_on_diff` | Different registrations → different fingerprint |
| `resolve_reusing_skips_unchanged` | `resolve_reusing` skips re-creation of unchanged beans |

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
| `resolution_guard_detects_cycle` | Re-entrant `ResolutionGuard::enter` panics with cycle trace |
| `resolution_guard_cleans_up_on_drop` | Guard pops from thread-local stack on drop |
| `lazy_slot_resolves_once` | `LazySlot::get_or_init` runs factory exactly once |
| `lazy_slot_cached_after_first` | Second `get_or_init` returns cached value (no factory call) |
| `resolve_lazy_factory_multithread` | `resolve_lazy_factory` works on multi-thread runtime |
| `resolve_lazy_factory_control_plane` | Control-plane path spawns on control-plane handle |
| `resolve_lazy_factory_no_runtime_panics` | No runtime + no fallback feature → panic |
| `lazy_wrapper_get_resolves` | `Lazy<T>::get()` resolves on first call |
| `lazy_wrapper_clone_shares_cell` | `Lazy<T>::clone()` shares the inner `OnceCell` |

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
