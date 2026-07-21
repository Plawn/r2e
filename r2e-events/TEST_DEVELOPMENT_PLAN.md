# r2e-events — Test Development Plan

## Coverage Gaps (llvm-cov 2026-07-21)

### Summary

| Component | Line coverage | Lines covered | Uncovered |
|---|---|---|---|
| **Core** (`r2e-events`) | **73.4%** | 1037/1413 | 376 |
| **Kafka** backend | **18.3%** | 233/1273 | 1040 |
| **Iggy** backend | **9.5%** | 106/1111 | 1005 |
| **RabbitMQ** backend | **8.6%** | 90/1051 | 961 |
| **Pulsar** backend | **5.9%** | 55/934 | 879 |

### Core — uncovered files (sorted by gap)

| File | Coverage | Uncovered | What's missing |
|---|---|---|---|
| `src/backend/state.rs` | 69.0% | 159 | `BackendState` lifecycle: `register_poller_cancel`, `resolve_topic`, `is_topic_ensured`/`set_topic_ensured`, `register_handler` / `register_handler_inner`, `build_unsubscribe_handle`, `unregister_handler`, `configure_handler` (filter+retry), `invoke_with_retry`, `InFlightGuard`, `spawn_completion_forwarder` |
| `src/lib.rs` | 45.5% | 127 | `EmitReceipt` (new/ready/confirm), `SubscriptionHandle` (new/unsubscribe/id/Debug), `ResponderHandle` (new/unregister/Debug), `EventEnvelope`, `DlqPublisher` type alias, `EventBus` trait default impls (`register_topic`, `configure_handler`, `subscribe_with_deserializer`, `emit_nowait`, `emit_nowait_with`, `request`/`respond` defaults), `EventFilter`/`RetryPolicy` structs |
| `src/local.rs` | 83.8% | 52 | `emit_nowait` / `emit_nowait_with` impls, `clear()`, `shutdown(timeout)` drain logic, `Default` impl |
| `src/sse_bridge.rs` | 74.5% | 12 | `SseBridge` construction and drop paths |

### Core — missing tests

| Test | File | What it covers | Lines |
|---|---|---|---|
| `emit_receipt_ready_confirms_ok` | `event_bus.rs` | `EmitReceipt::ready().confirm()` returns Ok | ~5 |
| `emit_receipt_new_wraps_future` | `event_bus.rs` | `EmitReceipt::new(async { Ok(()) }).confirm()` resolves | ~5 |
| `emit_nowait_returns_receipt` | `event_bus.rs` | `bus.emit_nowait(event)` returns receipt, `.confirm()` works | ~10 |
| `emit_nowait_with_metadata` | `event_bus.rs` | `bus.emit_nowait_with(event, metadata)` returns receipt | ~10 |
| `clear_removes_all_handlers_and_responders` | `event_bus.rs` | After `bus.clear()`, emit delivers to nobody, respond is unregistered | ~10 |
| `shutdown_timeout_with_slow_handler` | `event_bus.rs` | `bus.shutdown(100ms)` with stuck handler → returns error, handlers cleared | ~15 |
| `shutdown_drains_fast_handlers` | `event_bus.rs` | `bus.shutdown(5s)` with fast handler → Ok, handlers cleared | ~10 |
| `subscription_handle_debug_format` | `event_bus.rs` | `format!("{:?}", handle)` contains "SubscriptionHandle" and id | ~3 |
| `event_filter_suppresses_non_matching` | `event_bus.rs` | `configure_handler` with filter → filtered events not dispatched | ~15 |
| `retry_policy_retries_on_nack` | `event_bus.rs` | `configure_handler` with retry → nack'd handler re-invoked | ~15 |
| `backend_state_register_poller_cancel` | `backend_state.rs` | `register_poller_cancel` returns token, `cancel_all_pollers` cancels it | ~10 |
| `backend_state_resolve_topic_cached` | `backend_state.rs` | Second `resolve_topic::<E>()` call returns same Arc (cache hit) | ~5 |
| `backend_state_ensure_topic_idempotent` | `backend_state.rs` | `set_topic_ensured` → `is_topic_ensured` returns true | ~5 |
| `backend_state_configure_handler_with_hint` | `backend_state.rs` | `configure_handler` with type_id_hint sets filter on matching handler | ~10 |
| `backend_state_configure_handler_fallback_scan` | `backend_state.rs` | `configure_handler` without hint finds handler by scanning all types | ~10 |
| `in_flight_guard_decrements_on_drop` | `backend_state.rs` | Drop InFlightGuard → in_flight counter decremented, notifies waiters | ~10 |
| `spawn_completion_forwarder_sends_outcome` | `backend_state.rs` | `spawn_completion_forwarder` → outcome received on channel | ~10 |

Estimated gain: **~148 lines** → core coverage **73.4% → ~83.9%**

### Backends — coverage gap analysis

All four backends share the same structural gap: the runtime code (connection, emit/subscribe loops, partition management, request-reply transport) requires a live broker. Only config/sanitize/codec/error paths are unit-testable.

| File | Coverage | Uncovered | Runtime code |
|---|---|---|---|
| `kafka/src/bus.rs` | 8.1% | 644 | `EventBus` trait impl: `subscribe`, `emit`, `emit_with`, `emit_nowait`, request/respond, poller loop, offset commits, rebalance, DLQ routing |
| `kafka/src/request.rs` | 0.0% | 203 | `run_reply_consumer`, reply routing, responder loop, pipelined offset commits |
| `kafka/src/config.rs` | 53.5% | 112 | `to_producer_client_config`, `to_consumer_client_config`, SASL/SSL config, validation |
| `kafka/src/builder.rs` | 21.6% | 80 | `connect()`: producer creation, Arc::new_cyclic, DLQ publisher wiring |
| `iggy/src/bus.rs` | 2.7% | 895 | Full EventBus impl: ensure_topic, subscribe poller, emit serialization, request-reply, shutdown drain |
| `iggy/src/builder.rs` | 0.0% | 93 | `connect()`: Iggy client creation, stream/topic bootstrap |
| `rabbitmq/src/bus.rs` | 0.0% | 629 | Full EventBus impl: declare exchange/queue/binding, publish, consume loop, ack/nack |
| `rabbitmq/src/inner.rs` | 0.0% | 280 | `RabbitMqInner` construction, channel rebuild, reconnect, AMQP properties |
| `pulsar/src/bus.rs` | 0.0% | 795 | Full EventBus impl: create producer/consumer, publish, consume, request-reply |
| `pulsar/src/builder.rs` | 0.0% | 58 | `connect()`: Pulsar client creation |

### Backends — testable without broker

| Test | Backend | What it covers |
|---|---|---|
| `kafka_config_to_producer_client_config` | kafka | `to_producer_client_config()` builds valid ClientConfig from KafkaConfig |
| `kafka_config_to_consumer_client_config` | kafka | `to_consumer_client_config()` includes group.id, auto.offset.reset |
| `kafka_config_sasl_wiring` | kafka | SASL mechanism/username/password mapped to rdkafka keys |
| `kafka_config_ssl_wiring` | kafka | SSL CA/cert/key paths mapped correctly |
| `kafka_reply_consumer_group_format` | kafka | `reply_consumer_group("g", 42)` → `"g.reply-consumer.000000000000002a"` |
| `rabbitmq_build_properties_persistent` | rabbitmq | `build_properties` with persistent=true → delivery_mode=2 |
| `rabbitmq_build_properties_headers` | rabbitmq | EventMetadata encoded into AMQP headers |
| `rabbitmq_connection_props_default_name` | rabbitmq | Default connection name = "r2e-events-rabbitmq" |
| `iggy_ensure_topic_skips_when_not_auto_create` | iggy | `auto_create=false` → `ensure_topic` is no-op |
| `pulsar_full_topic_cached` | pulsar | Second `full_topic("x")` returns same Arc (cache hit) |

### Backends — requires live broker (#[ignore])

These tests exist but are `#[ignore]`. To improve backend coverage, run them in CI with testcontainers or a docker-compose sidecar.

| Test scope | Backends | Estimated coverage gain |
|---|---|---|
| `connect` → `emit` → `subscribe` → receive roundtrip | all 4 | +30-40% per backend |
| `request` → `respond` roundtrip | kafka, iggy, rabbitmq | +10-15% per backend |
| `emit_nowait` + `confirm` | kafka, iggy | +5% per backend |
| Reconnect after broker restart | kafka, rabbitmq | +5-10% per backend |
| Graceful shutdown drain | all 4 | +5% per backend |

---

## Phase 1: Error & Panic Isolation (Critical) ✅ DONE

**File**: `tests/event_bus.rs`

| Test | Description | Status |
|------|-------------|--------|
| `handler_panic_does_not_crash_emit` | Handler panicking in `emit()` → bus still functional | ✅ |
| `handler_panic_does_not_crash_drain` | Handler panicking in `drain()` → returns without panic | ✅ |
| `panic_releases_permit` | Handler panic with concurrency=1 → permit released, next handler runs | ✅ |
| `multiple_handlers_one_panics` | 3 handlers, 1 panics → other 2 still execute | ✅ |
| `err_result_in_handler` | Handler returning `Err` internally → no effect on bus | ✅ |

---

## Phase 2: Subscription Safety ✅ DONE

| Test | Description | Status |
|------|-------------|--------|
| `late_subscriber_misses_event` | Subscribe after `emit()` → does not receive past event | ✅ |
| `concurrent_subscribes` | 10 threads subscribing simultaneously → all registered | ✅ |
| `subscribe_during_emit` | `subscribe()` while `emit()` is in-flight → no panic, consistent state | ✅ |
| `subscribe_same_event_type_multiple` | Multiple subscriptions for same `TypeId` → all called | ✅ |

---

## Phase 3: Edge Cases & Lifecycle ✅ DONE

| Test | Description | Status |
|------|-------------|--------|
| `emit_no_subscribers` | `emit()` on bus with zero subscribers → instant, no error | ✅ |
| `drain_no_subscribers` | `drain()` with zero subscribers → instant return | ✅ |
| `default_eventbus` | `EventBus::default()` equivalent to `EventBus::new()` | ✅ |
| `concurrency_limit_bounded` | `EventBus::with_concurrency(5).concurrency_limit()` → `Some(5)` | ✅ |
| `concurrency_limit_unbounded` | `EventBus::unbounded().concurrency_limit()` → `None` | ✅ |
| `clone_shares_state` | Cloned bus shares subscribers with original | ✅ |
| `drop_bus_with_active_handlers` | Drop `EventBus` while handlers running → no panic | ✅ |

---

## Phase 4: Async Handler Behavior ✅ DONE

| Test | Description | Status |
|------|-------------|--------|
| `handler_with_long_sleep` | `emit()` returns immediately despite slow handler | ✅ |
| `drain_waits_for_slow` | `drain()` blocks until slow handler completes | ✅ |
| `handler_spawns_nested_emit` | Handler calling `bus.emit()` internally → no deadlock | ✅ |
| `handler_shared_state_mutation` | Multiple handlers modifying `Arc<Mutex<_>>` → consistent final state | ✅ |

---

## Phase 5: Stress & Performance ✅ DONE

| Test | Description | Status |
|------|-------------|--------|
| `stress_many_events` | Emit 100 events → all delivered to subscriber | ✅ |
| `stress_many_subscribers` | 50 subscribers → all receive event | ✅ |
| `stress_concurrent_emit` | 10 threads emitting simultaneously → no data loss | ✅ |
| `backpressure_high_load` | Concurrency=2 with 50 events → max 2 concurrent at any time | ✅ |

---

## Phase 6: Consumer Integration (via example-app) ✅ DONE

**File**: `example-app/tests/consumer_test.rs`

| Test | Description | Status |
|------|-------------|--------|
| `consumer_method_invoked` | Emit event → `#[consumer]` method called | ✅ |
| `consumer_receives_correct_data` | Event payload accessible in consumer | ✅ |
| `consumer_with_injected_deps` | Consumer uses `#[inject]` fields from state | ✅ |
| `multiple_consumers_same_event` | Two controllers consuming same event type → both invoked | ✅ |
| `core_reused_across_consumer_invocations` | Same Arc<Core> used for every consumer call | ✅ |

---

## Phase 7: Request-Reply ✅ DONE

| Test | Description | Status |
|------|-------------|--------|
| `request_respond_roundtrip` | request→respond round-trip returns correct reply | ✅ |
| `request_without_responder_is_no_responder` | request without registered responder → NoResponder | ✅ |
| `responder_error_maps_to_remote` | responder returning Err → Remote error | ✅ |
| `request_times_out_when_responder_is_slow` | slow responder → RequestTimeout | ✅ |
| `second_responder_for_same_type_is_rejected` | duplicate respond → AlreadyRegistered | ✅ |
| `unregister_responder_allows_reregistration` | unregister → re-register succeeds | ✅ |

---

## Phase 8: BackendState & Watermark ✅ DONE

| Test | Description | Status |
|------|-------------|--------|
| 21 tests in `backend_state.rs` | Tracked/untracked dispatch, DLQ, responder invoke, build_reply | ✅ |
| 12 tests in `backend_watermark.rs` | In/out-of-order acks, nack pinning, partition isolation, rebalance | ✅ |

---

## Phase 9: Core coverage gaps (NEW — from llvm-cov)

Target: **73.4% → ~84%**

| Test | File | What it covers |
|------|------|---------------|
| `emit_receipt_ready_confirms_ok` | `event_bus.rs` | `EmitReceipt::ready().confirm()` returns Ok |
| `emit_receipt_new_wraps_future` | `event_bus.rs` | `EmitReceipt::new(async { Ok(()) }).confirm()` resolves |
| `emit_nowait_returns_receipt` | `event_bus.rs` | `emit_nowait` returns receipt, `.confirm()` works |
| `emit_nowait_with_metadata` | `event_bus.rs` | `emit_nowait_with(event, metadata)` returns receipt |
| `clear_removes_all_handlers_and_responders` | `event_bus.rs` | After `clear()`, emit delivers to nobody |
| `shutdown_timeout_with_slow_handler` | `event_bus.rs` | `shutdown(100ms)` with stuck handler → error |
| `shutdown_drains_fast_handlers` | `event_bus.rs` | `shutdown(5s)` with fast handler → Ok |
| `event_filter_suppresses_non_matching` | `event_bus.rs` | `configure_handler` with filter → filtered events not dispatched |
| `retry_policy_retries_on_nack` | `event_bus.rs` | `configure_handler` with retry → nack'd handler re-invoked |
| `backend_state_register_poller_cancel` | `backend_state.rs` | `register_poller_cancel` returns token, cancel works |
| `backend_state_resolve_topic_cached` | `backend_state.rs` | Second `resolve_topic::<E>()` returns same Arc |
| `backend_state_ensure_topic_idempotent` | `backend_state.rs` | `set_topic_ensured` → `is_topic_ensured` true |
| `backend_state_configure_handler_with_hint` | `backend_state.rs` | configure_handler with type_id_hint sets filter |
| `backend_state_configure_handler_fallback_scan` | `backend_state.rs` | configure_handler without hint scans all types |
| `in_flight_guard_decrements_on_drop` | `backend_state.rs` | Drop InFlightGuard → counter decremented |
| `spawn_completion_forwarder_sends_outcome` | `backend_state.rs` | forwarder → outcome received on channel |

---

## Phase 10: Backend unit tests (NEW — no broker needed)

| Test | Backend | What it covers |
|------|---------|---------------|
| `kafka_config_to_producer_client_config` | kafka | `to_producer_client_config()` builds valid ClientConfig |
| `kafka_config_to_consumer_client_config` | kafka | `to_consumer_client_config()` includes group.id |
| `kafka_config_sasl_wiring` | kafka | SASL mechanism/username/password mapped correctly |
| `kafka_config_ssl_wiring` | kafka | SSL CA/cert/key paths mapped correctly |
| `kafka_reply_consumer_group_format` | kafka | reply_consumer_group format matches expected pattern |
| `rabbitmq_build_properties_persistent` | rabbitmq | persistent=true → delivery_mode=2 |
| `rabbitmq_build_properties_headers` | rabbitmq | EventMetadata encoded into AMQP headers |
| `rabbitmq_connection_props_default_name` | rabbitmq | Default connection name |
| `iggy_ensure_topic_skips_when_not_auto_create` | iggy | auto_create=false → no-op |
| `pulsar_full_topic_cached` | pulsar | Second full_topic returns same Arc |

---

## Estimated Effort

| Phase | Tests | Effort | Status |
|-------|-------|--------|--------|
| Phase 1 | 5 | 1.5h | ✅ Done |
| Phase 2 | 4 | 1.5h | ✅ Done |
| Phase 3 | 7 | 1h | ✅ Done |
| Phase 4 | 4 | 1.5h | ✅ Done |
| Phase 5 | 4 | 1h | ✅ Done |
| Phase 6 | 5 | 2h | ✅ Done |
| Phase 7 | 6 | 2h | ✅ Done |
| Phase 8 | 33 | 3h | ✅ Done |
| Phase 9 | 16 | 3h | TODO |
| Phase 10 | 10 | 2h | TODO |
| **Total** | **94** | **~18.5h** | |
