# EventBus Performance & Reliability Roadmap

Status: **IN PROGRESS** (validated 2026-07-12; P1 + a 10-finding review-fix
pass landed on branch `events/p1-ack-after-handler` — shared
`WatermarkTracker`/`spawn_completion_forwarder`, rebalance-aware Kafka
tracking, manual-commit progress, RabbitMQ ported to the shared engine
(P4.1), poison→DLQ parking, emit_and_wait outcome recorded for the poller.
P2 landed 2026-07-12: u128 event ids, RabbitMQ reconnect + per-consumer
channels, Pulsar per-topic producer locks, Kafka non-blocking shutdown
flush. P2.5 resolved and landed 2026-07-13: Vert.x-pure API —
emit_and_wait removed, request/respond on all backends + #[consumer]
responder sugar + shared reconnect_loop; see the P2.5 resolution section).
P3 landed 2026-07-13: `emit_nowait`/`emit_nowait_with` on `EventBus` trait
+ all backends (Kafka `send_result` + batching config, RabbitMQ
`confirm_select` + pipelined confirms, Pulsar receipt wrapping +
`tls_hostname_verification` wired, Iggy spawn-based nowait); `EmitReceipt`
with optional `confirm()`. Dead Pulsar `batch_size` field removed.
Check items off as they land. Hub referenced from `roadmap.md` (W8).

Scope: `r2e-events` (LocalEventBus + shared `backend` module) and the four
distributed backends (`backends/iggy`, `backends/kafka`, `backends/pulsar`,
`backends/rabbitmq`).

## Audit verdict (2026-07-12)

- **LocalEventBus: sound.** Standard snapshot-then-dispatch design, real
  backpressure, panic-safe in-flight tracking, no locks across awaits. Only
  micro-optimizations and one sharded-mode question remain.
- **Shared `BackendState`: well designed** (permit-before-spawn backpressure,
  RAII in-flight guard, bounded dedup set) — but carries one real
  correctness bug (P2.1) and per-message overheads (P5).
- **All four distributed backends: not production-grade as-is.** Two shared
  structural defects: (a) one awaited network round-trip per `emit` with no
  batching/fire-and-forget path → sequential emit throughput ≈ 1/RTT;
  (b) three of four ack/commit **before** the handler runs → effective
  at-most-once with silent loss on crash, while the retry/DLQ machinery
  implies at-least-once. Plus serious backend-specific bugs (RabbitMQ
  reconnect, Pulsar global producer lock).

Execution order: **P1 (semantics) → P2 (reliability bugs) → P3 (producer
throughput) → P4 (consumer throughput) → P5 (micro-optimizations)**.
P1 and P2 are correctness; P3–P5 are performance. Breaking changes are fine
(R2E is pre-production) — call them out in PR descriptions.

---

## P1 — Delivery semantics: commit/ack AFTER handler completion

**Decision to make first** (single decision, applies to all backends):
target **at-least-once** delivery. Ack/commit only after local handlers
resolve; nack / skip commit on failure so the broker redelivers. The current
state — at-most-once commit timing combined with in-process retry/DLQ that
simulates at-least-once — is the worst of both.

Consequences to accept & document: handlers must be idempotent; redelivery
after crash is expected; per-message handler outcome must flow back to the
poller (dispatch can no longer be pure fire-and-forget on the consume path —
see P4 for keeping this pipelined rather than serial).

- [x] **P1.1 Shared dispatch: outcome-aware variant.** Add a
      `dispatch_from_poller`-style path in `backend/state.rs` that returns a
      per-message completion (all-Ack / any-Nack) without serializing the
      poll loop: spawn handler tasks as today, but hand the poller a future
      (or channel) resolving when all handlers for that message finish. The
      poller acks/commits from that signal — pipelined, permit-bounded.
- [x] **P1.2 Kafka: manual offset store.** `enable.auto.commit=false` +
      `store_offset` after successful dispatch, periodic commit (or
      auto-commit with manual store). Today: librdkafka auto-commit commits
      at `recv()` time regardless of handler outcome
      (`backends/kafka/src/config.rs:126`, consume loop
      `backends/kafka/src/bus.rs:407-409`).
- [x] **P1.3 Iggy: commit after consume.** Replace
      `AutoCommit::When(PollingMessages)` (`backends/iggy/src/bus.rs:432`)
      with `ConsumingAllMessages` or manual commit after dispatch completes.
- [x] **P1.4 Pulsar: ack after handler.** Move `consumer.ack(&received)`
      (`backends/pulsar/src/bus.rs:443-446`) behind the P1.1 completion
      signal; negative-ack on failure for redelivery.
- [x] **P1.5 RabbitMQ: keep ack-after-handler, drop the serial loop.**
      RabbitMQ is the only backend that already acks on handler outcome —
      but at the cost of a strictly serial consume loop
      (`backends/rabbitmq/src/bus.rs:549-561`). Port it onto P1.1 so it
      pipelines (tracked as P4.1; P1.5 is just "don't regress semantics
      while fixing P4.1").
- [x] **P1.6 Docs.** State the delivery guarantee (at-least-once, idempotent
      handlers) in crate docs + `docs/claude/subsystems.md` EventBus section.

## P2 — Reliability bugs (fix regardless of P1 timing)

- [x] **P2.1 Cross-process `event_id` collision in the dedup set.** Fixed:
      `EventMetadata::event_id` is now a globally-unique `u128` = per-process
      random 64-bit identity (high bits, drawn once from a seeded `RandomState`
      hasher + wall-clock nanos + a stack address — no new dependency) packed
      with the per-process `AtomicU64` counter (low bits) via
      `compose_event_id`. The codec (`src/backend/metadata_codec.rs`) now
      encodes/decodes the id as a decimal `u128` string (wire header
      `r2e-event-id` widened from u64 to u128 range), and the dedup set
      (`LocallyDispatchedSet` in `src/backend/state.rs`) keys on `u128`.
      Distinct instances no longer collide, so a poller never drops a peer's
      message. Tests in `tests/event_id.rs` assert cross-process uniqueness
      and codec round-trip of a high-bit-set id.
- [x] **P2.2 RabbitMQ: reconnect never reconnects.** Fixed: `RabbitMqInner`
      now retains the `Connection` behind a mutex; `create_channel` reconnects
      it transparently when the link is down (serialized, so concurrent
      callers open at most one new connection). The consumer loop creates a
      fresh channel + re-declares its queue on each (re)connect, and now
      **breaks** on a stream-level `Err` (previously it slept and spun on the
      dead channel forever) so the backoff/reconnect path actually engages.
      The publisher channel is (re)created lazily via `publisher_channel`,
      recovering after a broker blip.
- [x] **P2.3 RabbitMQ: single shared channel for publish + all consumers.**
      Fixed together with P2.2: one dedicated channel **per consumer** (owned
      by `run_consumer_inner`, dropped/closed only when that consumer exits)
      plus a **separate** lazily-created publisher channel. A failed publish
      now only tears down the publisher channel; consumers are unaffected.
- [x] **P2.4 Pulsar: global producer mutex held across `.await`.** Fixed:
      the producer map is now `Mutex<HashMap<String, Arc<Mutex<Producer>>>>`.
      The map lock is held only for the lookup/insert (never across a broker
      connect or a send); producers are built outside the map lock with a
      double-checked `entry().or_insert_with` (a losing racer's producer is
      dropped cleanly). Sends serialize per topic only, and the receipt is
      still awaited with no lock held.
- [x] **P2.5 `emit_and_wait` on distributed backends: cross-instance double
      processing.** **RESOLVED by decision 2026-07-13 (user): Vert.x-pure
      API, LANDED same day** — see the "P2.5 resolution" section below.
      `emit_and_wait` removed entirely (with the dispatch-local +
      `locally_dispatched` dedup machinery; P5.3 moot); `request`/`respond`
      implemented on LocalEventBus + all four backends + `#[consumer]`
      responder sugar. A review-fix pass hardened it: reply-publish failure
      no longer acks/commits the request (kafka/pulsar/rabbitmq), Pulsar
      reply consumer starts at Earliest, per-instance (not per-process)
      reply topics, dedicated `r2e-request-id` header so the user's
      correlation_id survives, no-responder always yields an error reply.
- [x] **P2.6 Kafka: blocking `producer.flush` in async shutdown.** Fixed:
      shutdown flush now runs in `spawn_blocking` (flush errors still
      propagate; a JoinError is logged and swallowed). Noted but not fixed:
      the final `commit_consumer_state(CommitMode::Sync)` in the consumer
      drain tail is also blocking, but moving it needs care around the
      `StreamConsumer` borrow + manual-commit invariants — possible follow-up.

## P2.5 resolution — Vert.x-pure API (decided 2026-07-13)

Industry survey conclusion: no established system offers "emit and await all
subscribers" in distributed mode. Vert.x/Quarkus has `publish` (fan-out,
fire-and-forget, no reply possible) and `request` (point-to-point, ONE
consumer replies, timeout); Spring's synchronous `publishEvent` is strictly
in-process; broker systems only await the broker ack. The user chose the
**Vert.x-pure** model over the Spring hybrid:

- **`emit_and_wait` / `emit_and_wait_with` are REMOVED from the `EventBus`
  trait and every implementation.** `emit` stays fan-out with no handler
  await. Tests that need determinism use `request` or in-flight draining.
- **The entire local-echo machinery goes with it:** `LocallyDispatchedSet`
  in `src/backend/state.rs`, `record_local_dispatch`, the poller-side dedup
  check, and the dispatch-local-before-publish path from the P1 pass. One
  delivery path per consumer group — the P2.5 double-processing problem is
  removed, not patched. P5.3 is moot.
- **New `request<Req, Resp>` / `respond<Req, Resp>` API** (all backends):
  - `bus.request(req) -> Result<Resp, EventBusError>` — point-to-point,
    awaits ONE responder's reply. Default timeout 30s, configurable per
    backend (`request_timeout`); `request_with` takes explicit
    timeout/metadata. Errors: `NoResponder`, `RequestTimeout`,
    `Remote(String)` (responder returned an error — Vert.x ReplyException
    equivalent).
  - `bus.respond(handler)` — registers the responder for `Req`. **At most
    one responder per request type per process** (second registration
    errors). Cross-instance load balancing comes from the broker
    (queue/consumer-group semantics), not in-process round-robin.
  - Transport: Local = direct call. RabbitMQ = classic RPC (Direct
    Reply-To `amq.rabbitmq.reply-to` + correlation_id). Kafka/Pulsar/Iggy =
    shared request topic + per-instance reply topic
    (`<prefix>.replies.<process-id>`) + correlation header
    (ReplyingKafkaTemplate pattern). Correlation ids reuse the u128
    `event_id` scheme from P2.1.
- **Macro sugar (same pass):** a `#[consumer]` method with a non-`()`
  return type becomes a responder (Quarkus `@ConsumeEvent`-style — the
  return value IS the reply); wired through routes codegen +
  `register_controller`.

Execution waves: (1) shared crate — trait change, LocalEventBus,
removal of the dedup machinery, shared request/reply plumbing in
`src/backend/` (pending-request correlation map, reply metadata headers);
(2) four backend agents in parallel; (3) macros + example-app, parallel
with (2); (4) review pass + docs (book `event-bus.md`,
`features/07-evenements.md`, `subsystems.md`, root `CLAUDE.md` crate
description) + commits.

## P3 — Producer throughput (unblock >1/RTT emit)

Shared problem: every `emit` serializes with serde_json and awaits one full
broker round-trip. A sequential `for e in batch { bus.emit(e).await }` caps
at a few hundred msg/s on every backend.

- [x] **P3.1 API: decide the shape of fast emit.** Decision: option (a) —
      `emit` stays awaited-durable, new `emit_nowait` / `emit_nowait_with`
      return `EmitReceipt` (drop = fire-and-forget, `.confirm().await` =
      durable, collect + `try_join_all` = batch confirm). `EmitReceipt` in
      shared `r2e-events` crate; trait has default impls delegating to `emit`.
- [x] **P3.2 Kafka.** `emit_nowait` uses `send_result` (non-blocking enqueue
      into librdkafka's producer buffer); wraps the `DeliveryFuture` in
      `EmitReceipt`. Surfaced first-class config: `linger_ms`, `batch_size`,
      `queue_buffering_max_messages`, `queue_buffering_max_kbytes`,
      `message_timeout_ms`, `enable_idempotence` (all `Option`, default =
      librdkafka default; `overrides` keeps final precedence).
- [x] **P3.3 Iggy: spawn-based nowait.** `emit_nowait` spawns the
      `send_messages` call in a background task and returns an `EmitReceipt`
      backed by a oneshot. Unblocks the caller immediately. Full channel +
      background flush batcher (coalescing same-topic messages) deferred to
      a follow-up if spawn-per-emit throughput is insufficient.
- [x] **P3.4 RabbitMQ: real publisher confirms.** `confirm_select` now called
      on every publisher channel (re)creation — `PublisherConfirm` resolves
      with an actual broker ack (not `NotRequested`). `emit` correctly awaits
      the confirm (truly durable now). `emit_nowait` returns the
      `PublisherConfirm` wrapped in `EmitReceipt` without awaiting it.
- [x] **P3.5 Pulsar: dead config cleanup + nowait.** `tls_hostname_verification`
      wired to the Pulsar client builder. Dead `batch_size` field removed
      (was consumer-named but unused). `auto_create`, `default_partitions`
      documented as reserved for future admin API. `emit_nowait` wraps the
      `SendFuture` receipt in `EmitReceipt` without awaiting it.

## P4 — Consumer throughput

- [x] **P4.1 RabbitMQ: pipeline the consume loop.** Done with the P1
      review-fix pass: the loop now uses `dispatch_from_poller_tracked` and a
      per-delivery task acks/nacks via lapin's owned `Acker`, bounded by
      prefetch + the shared semaphore. The unconditional `payload.to_vec()`
      DLQ copy is gone (shared `has_dlq` guard applies). Poison messages and
      DLQ-captured nacks now ack (previously requeued forever).
- [ ] **P4.2 Iggy: retune poll defaults + partitions.**
      `poll_interval=100ms` × `poll_batch_size=100`
      (`backends/iggy/src/config.rs:53-54`) caps a poller at ~1k msg/s and
      adds up to 100ms latency; `default_partitions=1`
      (`config.rs:36`) makes the consumer group unscalable (horizontal
      scale-out adds zero parallelism). Poll back-to-back while batches are
      full (interval only as idle backoff), raise batch size, raise/require
      partition count, document parallelism = min(partitions, consumers).
- [ ] **P4.3 Kafka/Pulsar ack-commit batching.** After P1: batch offset
      stores (Kafka) / acks (Pulsar — verify client-side ack buffering,
      consider cumulative ack) so ack traffic doesn't serialize the loop.
- [ ] **P4.4 (evaluate) Kafka: one `StreamConsumer` per event type**
      (`backends/kafka/src/bus.rs:376`) — N types = N connections + fetch
      loops. Optionally multiplex several topics onto one consumer with a
      topic→type dispatch map. Measure before doing.

## P5 — Hot-path micro-optimizations (after P3/P4 unblock real throughput)

Shared (`src/backend/`):

- [ ] **P5.1** `resolve_topic` clones a `String` per emit
      (`state.rs:210-215`, `topic.rs:24-29`) → store/return `Arc<str>`;
      cache per-TypeId resolved (full) topic names where backends re-derive
      them (Pulsar `full_topic_name` `format!` per emit,
      `backends/pulsar/src/bus.rs:97`; Iggy re-parses
      `Identifier::named(stream/topic)` per publish,
      `backends/iggy/src/bus.rs:125-127`).
- [ ] **P5.2** `encode_metadata`/decode round-trips: drop the intermediate
      `Vec<(String,String)>` — encode directly into the backend's header
      type, decode borrowing `&str` (`src/backend/metadata_codec.rs:11-75`;
      per-backend extract fns each allocate a second Vec + to_string per
      header).
- [x] **P5.3** ~~Dedup-set fast path~~ — moot: the `locally_dispatched`
      dedup set was deleted entirely by the P2.5 Vert.x-pure pass (no
      local-echo suppression exists anymore).
- [ ] **P5.9** Responder throughput: the request-topic consumers process
      requests strictly sequentially on kafka/pulsar/iggy (await the user
      responder before the next receive) — one slow responder invocation
      head-of-line-blocks every queued request on that instance (RabbitMQ
      spawns per delivery). Needs bounded concurrent dispatch that still
      respects commit ordering (Kafka offsets). Flagged by the P2.5 review;
      deferred — fits the P4 consumer-throughput work.
- [ ] **P5.10** `respond` API polish: the handler must return
      `Result<Resp, String>`, so the macro codegen and every manual caller
      stringify errors themselves. Accepting `E: Display` and mapping to the
      remote-error string inside the events crate would single-source the
      flattening. Flagged by the P2.5 review; deferred (API polish).
- [ ] **P5.11** Reply-topic hygiene: per-instance reply topics
      (`<prefix>.replies.<instance-hex>`) accumulate on the broker across
      restarts (kafka/pulsar/iggy). Decide retention/cleanup (short
      retention config, non-persistent topics on Pulsar, or startup GC).
- [ ] **P5.4** Deserialize outside the handlers read lock: clone the
      `DeserializerFn` + handler list under the lock, drop it, then
      deserialize (`src/backend/state.rs:352,413`).
- [ ] **P5.5** `ensure_topic` steady-state lock: `is_topic_ensured` takes a
      global `Mutex<HashSet>` on every publish (`state.rs:220-222`) —
      per-topic cached flag / lock-free snapshot.
- [x] **P5.6b** Reconnect/backoff loop duplication — done with the P2.5
      pass: shared `reconnect_loop` driver in `src/backend/reconnect.rs`,
      adopted by every consumer/poller/responder/reply loop in all four
      backends (10 call sites).
- [ ] **P5.6** Typed error classification: Iggy/Kafka `map_*_error` classify
      by `msg.contains("connect")` substring matching
      (`backends/iggy/src/error.rs:7-11`, `backends/kafka/src/error.rs:6-16`)
      and the result drives reconnect decisions — match typed error variants
      instead.

LocalEventBus:

- [ ] **P5.7** Lock-free handler snapshot: replace
      `tokio::sync::RwLock<HashMap<TypeId, Vec<HandlerEntry>>>` read-per-emit
      (`src/local.rs:131`) with an `ArcSwap` snapshot (handler maps are
      read-mostly; subscribes happen at boot). Only if a bench justifies it.
- [ ] **P5.8** Sharded-mode placement: all local handlers run on the control
      plane via `rt::spawn_ctl` (`src/local.rs:159`, and backend pollers
      likewise) — with `server.workers = per-core`, event processing does
      not scale with workers. Decide: document as intended isolation, or
      spawn handlers on the emitting runtime. Design question — resolve
      with the user.

## Explicitly deferred (from the 2026-03 audit, still deferred)

- `Arc<EventMetadata>` to avoid N metadata clones per dispatch — revisit if
  headers/correlation_id get heavily populated with high fan-out.
- Lazy `EventMetadata::new()` — revisit only if a zero-alloc local dispatch
  path becomes a goal.

## Verification plan

- Unit/integration tests per phase in `<crate>/tests/` (repo convention).
- P1/P2 need failure-injection tests: kill handler mid-flight and assert
  redelivery (per backend, behind dev-services containers where available;
  RabbitMQ/Kafka land first via `r2e-devservices` W6 work).
- P3/P4 need a small throughput smoke bench (emit N=50k, measure wall clock,
  local broker containers) — not a CI gate, a before/after data point
  recorded in the PR.
