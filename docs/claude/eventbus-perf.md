# EventBus Performance & Reliability — open items

Workstream W8 (P1–P5) shipped via PR #30 (2026-07-14) plus follow-ups on
2026-07-20; the completed history has been pruned. What follows is what is
still open, plus the delivery-semantics context needed to reason about it.

Scope: `r2e-events` (LocalEventBus + shared `backend` module) and the four
distributed backends (`backends/iggy`, `backends/kafka`, `backends/pulsar`,
`backends/rabbitmq`).

## Reference — delivery semantics & shared architecture

- **Guarantee: at-least-once.** Every backend acks/commits only *after* local
  handlers resolve; failure means no commit / a nack, so the broker redelivers.
  Handlers must be idempotent. Documented in `r2e-events/src/lib.rs` crate docs
  and `docs/claude/subsystems.md`.
- **Shared dispatch:** `BackendState::dispatch_from_poller_tracked`
  (`r2e-events/src/backend/state.rs`) hands the poller a completion signal per
  message so ack/commit stays pipelined and permit-bounded (never a serial
  consume loop). Ordered offset commits go through
  `backend/watermark.rs`; reconnect/backoff through `backend/reconnect.rs`.
- **API shape:** `emit` is awaited-durable fan-out; `emit_nowait`/
  `emit_nowait_with` return an `EmitReceipt` (drop = fire-and-forget,
  `.confirm().await` = durable). `emit_and_wait` does not exist — the API is
  Vert.x-pure: `publish`-style `emit` + point-to-point `request`/`respond`
  (one responder per request type per process; broker load-balances across
  instances via a deterministic group derived from the request topic).
  Do not re-propose "emit and await all subscribers" for distributed mode —
  rejected 2026-07-13 after an industry survey.

## Open items

- [ ] **P4.4 (evaluate) Kafka: one `StreamConsumer` per event type.**
      `subscribe` spawns a dedicated `run_consumer` (and thus a
      `StreamConsumer`) on the first subscriber of each type
      (`backends/kafka/src/bus.rs:382-428`, consumer built at
      `bus.rs:789`) — N types = N connections + fetch loops. Optionally
      multiplex several topics onto one consumer with a topic→type dispatch
      map. **Measure before doing.**
- [ ] **Kafka: blocking `commit_consumer_state(CommitMode::Sync)` in the
      consumer drain tail** (`backends/kafka/src/bus.rs:898`). The shutdown
      producer flush was moved to `spawn_blocking` (`bus.rs:739`) but this
      final commit still blocks the async drain; moving it needs care around
      the `StreamConsumer` borrow + manual-commit invariants.
- [ ] **Iggy: real producer batching for `emit_nowait`.** Currently a
      spawn-per-emit backed by a oneshot (`backends/iggy/src/bus.rs:451`).
      A channel + background flush batcher coalescing same-topic messages was
      deferred until spawn-per-emit throughput is shown insufficient.

## Verification gaps (still open)

- **Failure-injection redelivery tests**: kill a handler mid-flight and assert
  redelivery, per backend, behind dev-services containers. The four backend
  `integration` features currently only cover live request/reply round-trips
  (e.g. `backends/kafka/tests/kafka_event_bus.rs:434`).
- **Throughput smoke bench** (emit N=50k against a local broker container, a
  before/after data point rather than a CI gate) was never recorded — needed
  before acting on P4.4 or the Iggy batcher.
