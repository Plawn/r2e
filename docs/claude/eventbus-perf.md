# EventBus Performance & Reliability Roadmap

Status: **IN PROGRESS** (validated 2026-07-12; P1 + a 10-finding review-fix
pass landed on branch `events/p1-ack-after-handler` — shared
`WatermarkTracker`/`spawn_completion_forwarder`, rebalance-aware Kafka
tracking, manual-commit progress, RabbitMQ ported to the shared engine
(P4.1), poison→DLQ parking, emit_and_wait outcome recorded for the poller).
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

- [ ] **P2.1 Cross-process `event_id` collision in the dedup set.**
      `EventMetadata::event_id` is a process-local `AtomicU64` starting at 1
      (`src/lib.rs:88,109`); the `emit_and_wait` dedup set keys on that u64
      (`src/backend/state.rs:342,401`). Two instances of the same app
      generate colliding ids → instance B's poller silently DROPS instance
      A's message when B has the same id pending, then double-dispatches its
      own. Near-certain in multi-instance deployments. Fix: globally unique
      event id — `(process_uuid, counter)` or a u128/UUID; adjust
      `EventMetadata`, the codec (`src/backend/metadata_codec.rs`), and the
      dedup set key. Breaking change to `EventMetadata` is fine.
- [ ] **P2.2 RabbitMQ: reconnect never reconnects.** The `Connection` is
      dropped at the end of `connect` (`backends/rabbitmq/src/builder.rs:67`,
      channel created once at `:74`); on disconnect `run_consumer` re-invokes
      `basic_consume` on the same dead channel forever
      (`backends/rabbitmq/src/bus.rs:419-449,459`). Any broker blip
      permanently kills publish + consume. Fix: retain the `Connection`,
      recreate channel (or connection) inside the retry loop.
- [ ] **P2.3 RabbitMQ: single shared channel for publish + all consumers**
      (`backends/rabbitmq/src/inner.rs:10`). One failed publish closes the
      channel and takes down every consumer. Fix: dedicated channel per
      consumer + a publisher channel (small pool optional). Do together with
      P2.2.
- [ ] **P2.4 Pulsar: global producer mutex held across `.await`.** One
      `Mutex<HashMap<String, Producer>>` (`backends/pulsar/src/inner.rs:17`)
      is held across `send_non_blocking(...).await`
      (`backends/pulsar/src/bus.rs:118-128`) and across producer
      build/connect (`bus.rs:66-81`): all emits on all topics serialize on
      one lock; the first emit to a new topic blocks everyone for a broker
      connect. Fix: `HashMap<String, Arc<Mutex<Producer>>>` — short map
      lock to clone the per-topic `Arc`, send under the per-topic lock only;
      build producers outside the map lock (double-checked insert).
- [ ] **P2.5 `emit_and_wait` on distributed backends: cross-instance double
      processing.** It dispatches locally AND publishes to the broker; the
      dedup set only suppresses the echo in the SAME process — another
      consumer-group member will process the broker copy too. Decide:
      accept + document ("local handlers run synchronously, group delivery
      still happens once somewhere"), or change semantics (e.g. don't
      publish, or don't locally dispatch, on distributed backends). Design
      decision — resolve with the user before implementing.
- [ ] **P2.6 Kafka: blocking `producer.flush` in async shutdown**
      (`backends/kafka/src/bus.rs:330`) — wrap in `spawn_blocking`.
      Shutdown-only, low priority but trivial.

## P3 — Producer throughput (unblock >1/RTT emit)

Shared problem: every `emit` serializes with serde_json and awaits one full
broker round-trip. A sequential `for e in batch { bus.emit(e).await }` caps
at a few hundred msg/s on every backend.

- [ ] **P3.1 API: decide the shape of fast emit.** Options: (a) `emit` stays
      awaited-durable and add `emit_nowait`/batched variant; (b) `emit`
      becomes enqueue-into-batcher and `emit_and_confirm` awaits the broker
      receipt. Pick once, apply to all backends + `EventBus` trait. (Trait
      change = breaking, fine.)
- [ ] **P3.2 Kafka.** Don't await the delivery future per message on the
      fast path (librdkafka batches internally); surface
      `linger.ms`, `batch.size`/`batch.num.messages`,
      `queue.buffering.max.*`, `message.timeout.ms`, `enable.idempotence`
      as first-class config (`backends/kafka/src/config.rs:141-164`
      currently exposes none — only raw `overrides`).
- [ ] **P3.3 Iggy: producer-side batcher.** Today one single-message
      `send_messages` per emit (`backends/iggy/src/bus.rs:143-147`).
      Add a channel + background flush task coalescing same-topic messages
      into one `send_messages` batch (size/linger thresholds).
- [ ] **P3.4 RabbitMQ: real publisher confirms.** `confirm_select` is never
      called, so the awaited `PublisherConfirm` resolves `NotRequested` —
      `persistent`/`durable` defaults advertise durability that doesn't
      exist (`backends/rabbitmq/src/bus.rs:177-180`). Enable confirms on the
      publisher channel and pipeline them (don't await per-message serially).
- [ ] **P3.5 Pulsar: wire the dead config + optional no-receipt emit.**
      `batch_size`, `auto_create`, `default_partitions`,
      `tls_hostname_verification` are parsed but never applied
      (`backends/pulsar/src/config.rs:45-51`); wire them into
      producer/consumer builders or delete them. After P2.4, concurrent
      emitters pipeline; optionally add a variant that doesn't await the
      receipt (folds into P3.1).

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
- [ ] **P5.3** Dedup-set fast path: skip the global `locally_dispatched`
      Mutex per consumed message when the set is empty (atomic counter
      check) — apps that never call `emit_and_wait` currently pay it for
      nothing (`src/backend/state.rs:342,401`).
- [ ] **P5.4** Deserialize outside the handlers read lock: clone the
      `DeserializerFn` + handler list under the lock, drop it, then
      deserialize (`src/backend/state.rs:352,413`).
- [ ] **P5.5** `ensure_topic` steady-state lock: `is_topic_ensured` takes a
      global `Mutex<HashSet>` on every publish (`state.rs:220-222`) —
      per-topic cached flag / lock-free snapshot.
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
