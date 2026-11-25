# r2e-core — mutation testing report (2026-11-25)

cargo-mutants 27.1.0, `r2e-core` at `c2131d58`, `--all-features`, tests =
`--lib --tests` of **r2e-core only** (no doctests, no downstream crates),
timeout 120 s. Before any fix.

| Outcome | Count |
|---|---|
| total | 2384 |
| caught | 1003 |
| missed | 554 (34 noise, 520 candidates) |
| timeout | 37 (real hangs → count as caught) |
| unviable | 790 |

Raw lists: `missed.txt`, `missed-noise.txt` (now excluded by
`.cargo/mutants.toml`), `missed-candidates.txt`, `timeout.txt`.

**Big caveat:** only r2e-core's own tests ran. Code exercised by
`r2e-security`, `r2e-openapi`, `r2e-test`, `example-app` shows as missed
here (e.g. `Audience::contains`, `body_schema.rs`, `launch`). Each gap
below must be confirmed with `--test-package r2e-core,<consumer>` (or
`--test-workspace true`) before writing a test.

## A. Real gaps — behaviour, worth a test (priority order)

1. **Security-sensitive (`decorators/claims.rs`, `guards.rs`)** —
   `Audience::contains` → `true`/`false`, `==`→`!=`; `Audience::iter`,
   `StandardClaims::{realm_roles, client_roles, scopes}` → empty; 
   `GuardContext::{forwarded_for, forwarded_ip}`,
   `PreAuthGuardContext::{path, query_string, path_param}`, `Identity::email`.
   Probably covered in r2e-security — **verify first**; if not, top priority.
2. **Error → status mapping (`error/rejection.rs`, `error/mod.rs`)** —
   every `From<FormRejection | StringRejection | RawFormRejection |
   BytesRejection | MultipartRejection>` arm/guard (413 / 415 / 400
   distinction), `Rejection::project` `!=` at 438;
   `HttpError::from_status` 400/403/500 arms, `status()`, `message()`.
3. **Body limits (`web/multipart.rs`)** — `collect_from_with_limits`
   comparisons at 177/183 and `+=` at 188 (limit off-by-one or disabled
   undetected), `take_text/_opt/file_opt/files/bytes`, `UploadedFile::{len,is_empty}`.
4. **Shutdown / drain (`builder/running.rs`, `prepared.rs`, `mod.rs`)** —
   `RunningApp::has_shutdown_work` and `Drop` (every `!`/`||` at 171–176,
   298–310), `track_named`, `serve_tracked`; `drain_tracked_handles` /
   `stop_post_drain_services` panic/cancel guards (1252–1312),
   `PluginShutdownCell::is_pending`, `disarm_ws_sessions`, `run_inner`
   951/1007–1013; `ServiceHandles::{push,has_live,abort_all}`,
   `PostDrainServices::{has_live,abort_all}`, `WsSessions::{disarm,is_armed}`,
   `TaskRegistryHandle::{add_boxed(_for),take_of,take_all}`.
5. **Boot entry points (`builder/app.rs`, `typed.rs`)** — `launch`,
   `launch_with`, `serve`, `serve_auto` → `Ok(())`; `boot_error_report`,
   `exit_on_boot_error`; `wire_controller_lifecycle` `!` at 860,
   `try_register_controller_unchecked_at` `>`→`>=` at 779.
6. **Bean graph (`beans/*`)** — `find_cycle` (DONE arm 339, 355, 367),
   `duplicate_error` 106, `overridable_indices_to_remove` 53,
   `register_plugin_group` `^`/`+` (44, 107), `register_scheduled_source` /
   `register_event_subscriber` / `register_service_source` /
   `register_deco_fill` (`==`→`!=`), `take_scheduled_sources`,
   `take_event_subscribers`, `is_pinned`→`true`, `set_panic_hook_slot`,
   `compute_fingerprint` `!` 117, `resolve_reusing` `!` 279,
   `BeanError::source` arms 235–238, `DecoSlot`/`SharedDecoSlot` fill/get/clone.
7. **Config (`config/*`)** — `de.rs` enum guard 258, `visit_seq` 309,
   `next_element_seed` 336, `unit_variant`; `value.rs` `f64` from int/string,
   bool `"true"|"1"|"yes"`, `PathBuf`, duration `"m"` unit (×60!);
   `loader.rs` `&&` 35/83; `tree.rs` 32/83/159/243; `secrets.rs` 160/184/186;
   `has_prefix` guard 494, `is_prefix`, `current_env_overlay_keys`;
   `registry.rs` `register_section`.
8. **Live config (`config/runtime.rs`)** — backoff `>=` 156, `last_error`,
   `is_stale`, `set_error` (×2), `snapshot`, `LiveConfigReceiver::{changed,drive}`.
9. **Health (`builtins/health.rs`)** — `aggregate` / `aggregate_readiness`
   `<` at 316/371 (worst-status ordering!), `affects_readiness`→`false`,
   `cache_ttl`, `names/len/is_empty`.
10. **WebSocket / SSE (`web/ws.rs`, `web/sse.rs`)** — `next_text`/`next_json`
    arms (Text/Close/Err), `send_json`, `send_binary`, `on_each`,
    `run_ws_handler`, `poll_close`, `protocol`, `shutdown_token`;
    `WsBroadcaster::{send, send_json, send_from, send_json_from, is_empty}`,
    `client_id`, `WsRooms::{reap_empty, is_empty}`;
    `SseBroadcaster::{len,is_empty}`, `SseTopic::{subscriber_count,capacity}`,
    `SseRooms::{reap_empty,is_empty}`.
11. **Tracing / http trace** — `install_tracing` 50/63/71, `install_summary`
    guards 109–140, `captured_headers` 421–436, `latency_ms` (`*`→`/`),
    `make_state`; `HttpTrace` / `HttpTraceBuilder` and `Cors` builder methods
    → `Default` (option silently dropped); `TracingConfig::{with_enabled,effective_format}`.
12. **Sharded runtime (`runtime/*`)** — `Mailboxes::broadcast(_with)` count,
    `missing` 314, `Mailbox::{try_recv,close,worker}`; `WorkerState::from_u8`
    arms; `WorkerSlot::{id,cpu,set_cpu}`, `wait_until`, `wait_all_exited`,
    `WorkerReadiness::check` `!=` 469, `WorkerHealth::build`;
    `ingress.rs` reuseport / cBPF attach (Linux-only — untestable on macOS);
    `sharded.rs` `parse_workers` 96, `ExitGuard::drop`, `serve_sharded` 381.
13. **Misc** — `di/module.rs` `join_path_prefix` guard 746, `di/lazy.rs`
    `ResolutionGuard::drop`, `plugin/install.rs` `&&` 495,
    `plugin/contexts.rs` / `deferred.rs` `on_shutdown_after_drain_async`,
    `mark_dev_reload_applied`, `runtime/dev.rs` `parse_port`,
    `invalidate_graph_fingerprint`, `request_head.rs` `query_string`/`extension`,
    `layers.rs` `is_end_stream`.

## B. Likely covered downstream (confirm, don't test in core)

- `web/body_schema.rs` (32), `error/schema.rs` (11), `di/meta.rs`
  `param_schema_of`, `web/params.rs` schema probes → r2e-openapi.
- `decorators/claims.rs` → r2e-security / r2e-mcp.
- `launch` / `serve*` → example-app, r2e-test.

## C. Equivalent / noise (no test can or should kill)

- `missed-noise.txt` (34): Debug/Display, `()`/`TNil` impls, `poll_ready`.
  Now excluded via `.cargo/mutants.toml`.
- Default trait methods returning `vec![]` (`Bean/AsyncBean/Producer::config_keys`,
  `Controller::{validate_config, on_start, scheduled_tasks_boxed}`,
  `ContextConstruct::config_keys`, `DecoratorSpec::config_*`,
  `ServiceComponent::config_*`, `ConfigProperties::properties_metadata`).
- `size_hint` (`config/de.rs` 344/379), log-only `log_service(s)_disabled`,
  pure accessors whose only test would be `assert_eq!(x.foo(), x.foo)`
  (`PreparedApp::{routes,addr,worker_set}`, `RunningApp::{routes, drain_timeout,…}`,
  `WorkerRole::as_str`, `type_name`, `route_label`) — test only where the
  value is consumed.

## D. Timeouts (37) — real hangs, count as caught

`web/ws.rs` (send/feed/close/next, broadcaster), `runtime/worker_set.rs`,
`runtime/sharded.rs`, `runtime/harness.rs`, `config/runtime.rs`
(`LiveConfigRegistry::{set,publish,is_pinned}`), `beans/resolve.rs` 78–87,
`builder/typed.rs` `wire_controller_lifecycle`. See `timeout.txt`.
