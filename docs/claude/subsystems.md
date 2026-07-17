# R2E Subsystems Reference

## AppBuilder (r2e-core)

Central orchestrator for assembling an R2E application. Two phases: pre-state and post-state.

```rust
AppBuilder::new()
    // ── Pre-state phase ──
    .plugin(Executor)                      // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)                     // scheduler runtime - MUST be before build_state()
    .load_config::<RootConfig>()             // load yaml + env, construct typed config, auto-register children (sole config entry)
    // test harness only: .override_config(cfg) BEFORE load_config stashes an in-memory R2eConfig it uses instead of disk
    .provide(services.pool.clone())        // provide beans
    .register::<CreatePool>()              // async producer (registers SqlitePool)
    .register::<MyAsyncService>()          // async bean constructor
    .register::<UserService>()             // sync bean — one unified register()
    // ── Conditional Self->Self assembly (plugins/layers, provision list P unchanged) ──
    .when(dev_mode, |b| b.with(DevReload))              // runtime bool
    .when(b.config_flag("metrics.enabled"), |b| b.with(Prometheus::default()))
    // For conditional *bean* presence: register a `#[producer] -> Option<T>`
    // (slot always in P; producer returns Some/None). config sections are
    // auto-registered as beans by load_config (inject with #[inject]).
    .build_state()                         // resolve bean graph → inferred HList state (async, no type args)
    .await                                 // .try_build_state().await is the non-panicking variant
    // ── Post-state phase ──
    .with(Health)                          // /health → 200 "OK"
    .with(Cors::permissive())              // or Cors::new(custom_layer)
    .with(Tracing)                         // default tracing (RUST_LOG only)
    // or: .with(Tracing::configured(config))  // with TracingConfig (format, ansi, etc.)
    // or: .with(Tracing::from_config(&r2e_config))  // from YAML tracing.* keys
    .with(ErrorHandling)                   // catch panics → JSON 500
    .with(DevReload)                       // /__r2e_dev/* endpoints
    .with(OpenApiPlugin::new(openapi_cfg)) // /openapi.json (+ /docs if docs_ui enabled)
    .on_start(|state| async move { Ok(()) })
    .on_stop(|state| async move { if let Some(p) = state.bean::<SqlitePool>() { p.close().await; } })
    .register_controller::<UserController>()
    .register_controller::<AccountController>()
    .register_controller::<ScheduledJobs>() // auto-discovers #[scheduled] methods
    // or register several at once: .register_controllers::<(UserController, AccountController, ScheduledJobs)>()
    // bean #[consumer] subscribers are auto-collected at build_state() — no explicit call
    .build()                               // → Router
    // or .serve("0.0.0.0:3000").await     // build + listen + graceful shutdown
    // or .serve_auto().await              // reads server.host / server.port from config (defaults: 0.0.0.0:3000)
```

**Lifecycle hooks** (post-state):
- `on_start(|state| async move { Ok(()) })` — runs before the server starts listening. Receives state, returns `Result`.
- `on_stop(|state| async move { })` — runs after graceful shutdown. Receives state, returns `()`.

`build()` returns a `Router` (from `r2e::http`). `serve(addr)` builds, runs startup hooks, registers event consumers, starts scheduled tasks, starts listening, waits for shutdown signal (Ctrl-C / SIGTERM), stops the scheduler, then runs shutdown hooks. `serve_auto()` does the same but reads address from config keys `server.host` (String, default `"0.0.0.0"`) and `server.port` (u16, default `3000`).

`.shutdown_grace_period(Duration)` — optional maximum time for shutdown hooks to complete before force-exiting the process. Without it, the process waits indefinitely.

`.r2e_config()` — returns `Option<&R2eConfig>`, available after `load_config()`. Used by `Tracing::from_config()` to read tracing settings from YAML.

## TracingConfig (r2e-core)

`TracingConfig` — `ConfigProperties` struct that configures the `tracing-subscriber` fmt layer. All fields except `filter` are `Option` — `None` means "use the subscriber default".

**YAML** (under a configurable prefix, e.g., `tracing.*` or `observability.tracing.*`):
```yaml
tracing:
  filter: "info,tower_http=debug"
  format: json          # pretty | json
  ansi: false
  target: true
  thread-ids: true
  thread-names: false
  file: true
  line-number: true
  level: true
  span-events: full     # none | new | close | active | full
```

**Programmatic API:**
- `TracingConfig::default()` — filter `"info,tower_http=debug"`, all other fields `None`
- Builder methods: `.with_format()`, `.with_filter()`, `.with_target()`, `.with_thread_ids()`, `.with_thread_names()`, `.with_file()`, `.with_line_number()`, `.with_level()`, `.with_ansi()`, `.with_span_events()`
- `effective_format()` → `LogFormat` (defaults to `Pretty`)
- `effective_span_events()` → `FmtSpan` (defaults to `CLOSE`)

**Related types:**
- `LogFormat` — `Pretty` (default) | `Json`. Derives `serde::Deserialize` + `FromConfigValue`.
- `SpanEvents` — `None` | `New` | `Close` (default) | `Active` | `Full`. `.to_fmt_span()` converts to `tracing_subscriber::fmt::format::FmtSpan`.

**Plugin integration:**
- `Tracing` (unit struct) — uses defaults (backward compatible)
- `Tracing::configured(TracingConfig)` → `ConfiguredTracing` — uses explicit config
- `Tracing::from_config(&R2eConfig)` → `ConfiguredTracing` — reads `tracing.*` keys
- `init_tracing_with_config(&TracingConfig)` — low-level function (idempotent)

**In ObservabilityConfig:**
`ObservabilityConfig` embeds `tracing: TracingConfig`. The `from_r2e_config()` loader reads from `observability.tracing.*`. Convenience method: `.with_log_format(LogFormat)` delegates to the embedded `TracingConfig`.

## ContextConstruct (r2e-core)

`ContextConstruct` trait allows constructing a controller core from the resolved bean graph alone (no HTTP context): `fn from_context(ctx: &BeanContext) -> Self` resolves each `#[inject]` field **by type** (`ctx.get::<T>()`) and each `#[config]` field from `R2eConfig`. Auto-generated by `#[controller]` for every controller core (always — the generated core holds `#[inject]` app fields, `#[config]` fields, and a hidden `DecoSlot` for `#[scheduled]`/`#[consumer]`-method interceptor sets; identity and request-scoped fields are stripped into the per-request façade). It replaces the removed `StatefulConstruct<S>` (which resolved from a hand-written state struct by field name). Required for:
- Consumer methods (`#[consumer]`) — event handlers that run outside HTTP requests
- Scheduled methods (`#[scheduled]`) — background tasks

Because the core never holds identity fields, `ContextConstruct` is available even on controllers that use struct-level or param-level `#[inject(identity)]` — consumers and scheduled tasks operate on the core.

## Configuration (r2e-core)

See [configuration.md](./configuration.md) for the full reference.

**AppBuilder integration** (pre-state methods):
- `load_config::<C>()` (the sole config registration point) — load YAML + env, construct typed config (`C: ConfigProperties`), **auto-register all nested `#[config(section)]` children as beans**, provide both `C` and `R2eConfig` in the type list. Use `load_config::<()>()` for raw only.
- `override_config(config)` — stash a pre-loaded/in-memory `R2eConfig` that the next `load_config` consumes instead of reading disk (test-harness primitive, not dev-reload plumbing — under `dev-reload`, `build()` re-runs per patch and its `load_config` re-reads `application.yaml`). Not a registration point on its own; `load_config` must still be called (else `build_state` panics).

Config sections registered via `load_config` are available as bean dependencies and for `#[inject]` in controllers.

## Security (r2e-security)

- `AuthenticatedUser` implements `FromRequestParts` and `Identity` — extracts Bearer token, validates via `JwtValidator`, returns user with sub/email/roles/claims.
- `JwtValidator` supports both static keys (testing) and JWKS endpoint (production) via `JwksCache`.
- `SecurityConfig` — configuration for JWT validation (issuer, audience, JWKS URL, static keys).
- `#[roles("admin")]` attribute generates a guard that checks identity roles via the `Identity` trait and returns 403 if missing.
- Role extraction is trait-based (`RoleExtractor`) to support multiple OIDC providers; default (`DefaultRoleExtractor`) checks top-level `roles` and Keycloak's `realm_access.roles`.

## Embedded OIDC (r2e-oidc)

`OidcServer` — embedded OAuth 2.0 / OIDC server plugin. Generates RSA-2048 keys, issues JWT tokens, exposes standard endpoints (`/oauth/token`, `/.well-known/openid-configuration`, `/.well-known/jwks.json`, `/userinfo`). Implements `PreStatePlugin` and provides `Arc<JwtClaimsValidator>` to the bean graph.

`OidcRuntime` — pre-built OIDC runtime (`Clone`). Created via `OidcServer::build()`. Holds all expensive state (`Arc`-wrapped RSA keys, user store, client registry). Reusable across hot-reload cycles — only re-registers routes without regenerating keys. Also implements `PreStatePlugin`.

Two usage patterns:
- **Simple:** `AppBuilder::new().plugin(OidcServer::new().with_user_store(users))` — generates keys on each install. Works without hot-reload.
- **Hot-reload:** `let oidc = OidcServer::build();` in `setup()`, then `.plugin(oidc.clone())` in `main(env)`. Tokens survive hot-patches.

Key types: `InMemoryUserStore`, `OidcUser`, `UserStore` trait, `ClientRegistry`, `OidcServerConfig`.

## Events (r2e-events)

`EventBus` — pluggable event bus **trait**. `LocalEventBus` — default in-process implementation. Events are dispatched by `TypeId`. Subscribers receive `EventEnvelope<E>` containing `Arc<E>` + `EventMetadata`.

**Core types:**
- `EventEnvelope<E>` — wraps `event: Arc<E>` + `metadata: EventMetadata`.
- `EventMetadata` — auto-generated per emit: `event_id`, `timestamp`, optional `correlation_id`, `partition_key`, `headers: HashMap<String, String>`.
- `HandlerResult` — `Ack` or `Nack(String)`. Implements `From<()>` and `From<Result<(), E>>`.
- `SubscriptionHandle` — returned by `subscribe()`, supports `unsubscribe()`.
- `EventBusError` — `Serialization`, `Connection`, `Shutdown`, `Other`, plus the request-reply variants `NoResponder`, `RequestTimeout`, `Remote(String)`.
- `RequestOptions` — controls a single `request_with` call: `with_timeout(Duration)` (default `DEFAULT_REQUEST_TIMEOUT` = 30s), `with_metadata(EventMetadata)`.
- `ResponderHandle` — returned by `respond()`; `unregister()` removes the responder so another may take its place.
- `Event` trait — opt-in trait with `fn topic() -> &'static str` for distributed backends.

**EventBus trait methods:**
- `bus.subscribe(|envelope: EventEnvelope<MyEvent>| async { HandlerResult::Ack })` → `Result<SubscriptionHandle, EventBusError>`. Requires `E: DeserializeOwned`.
- `bus.emit(event)` → `Result<(), EventBusError>`. Fan-out fire-and-forget (Vert.x `publish`): every subscriber gets a copy, no reply. Requires `E: Serialize`.
- `bus.emit_with(event, metadata)` → `Result<(), EventBusError>`. Emit with explicit metadata.
- `bus.emit_nowait(event)` → `Result<EmitReceipt, EventBusError>`. Enqueue without waiting for broker ack. The returned `EmitReceipt` lets the caller optionally `.confirm().await` later. Default trait impl delegates to `emit` then returns `EmitReceipt::ready()`.
- `bus.emit_nowait_with(event, metadata)` → `Result<EmitReceipt, EventBusError>`. Nowait emit with explicit metadata.
- `EmitReceipt` — opaque handle wrapping a boxed future. `.confirm()` awaits the broker ack. `EmitReceipt::ready()` is an already-resolved receipt (used by `LocalEventBus` and the default trait impl). `EmitReceipt::new(fut)` wraps any `Future<Output = Result<(), EventBusError>> + Send + 'static`.
- `bus.request(req)` → `Result<Resp, EventBusError>`. Point-to-point request-reply (Vert.x `request`): awaits the single responder's reply, 30s default timeout. Errors: `NoResponder` (local only — distributed backends surface an absent responder as `RequestTimeout`), `RequestTimeout`, `Remote(msg)` (responder returned `Err`).
- `bus.request_with(req, RequestOptions)` → `Result<Resp, EventBusError>`. Request with explicit timeout/metadata.
- `bus.respond(handler)` → `Result<ResponderHandle, EventBusError>`. Registers the single responder for `Req`; handler returns `Result<Resp, String>` (the `Ok` value is the reply, `Err(msg)` reaches the requester as `Remote(msg)`). At most one responder per request type per process — a second registration errors. Cross-instance load balancing comes from the broker (queue/consumer-group), not in-process round-robin.
- `bus.shutdown(timeout)` → `Result<(), EventBusError>`. Graceful shutdown: rejects new emits, waits for in-flight handlers.
- `bus.clear()` — remove all handlers.

A `#[consumer]` method with a non-`()` return type is macro sugar for a responder (Quarkus `@ConsumeEvent`-style): the return value IS the reply, registered via `respond`; a `-> ()` consumer stays a plain fan-out subscriber registered via `subscribe`.

Event types must derive `Serialize + Deserialize` (required by the trait for backend compatibility; `LocalEventBus` never actually serializes — zero overhead).

Distributed backends (Kafka, Pulsar, RabbitMQ, Iggy) implement the `EventBus` trait. Shared backend utilities are in `r2e_events::backend` — `TopicRegistry`, `BackendState`, `encode_metadata`/`decode_metadata`.

**`emit_nowait` per-backend implementation:** Kafka uses `FutureProducer::send_result()` (sync enqueue, `'static` `DeliveryFuture`); RabbitMQ wraps `PublisherConfirm` (channel has `confirm_select` enabled); Pulsar wraps `send_non_blocking`'s `SendFuture`; Iggy spawns a task + oneshot (SDK has no internal batcher). Kafka also exposes batching config: `linger_ms`, `batch_size`, `queue_buffering_max_messages`, `queue_buffering_max_kbytes`, `message_timeout_ms`, `enable_idempotence`.

**Delivery semantics (distributed backends): at-least-once.** The broker copy is acked/committed only after all local handlers for the message resolve (`BackendState::dispatch_from_poller_tracked` → `DispatchCompletion::outcome()` → `DispatchOutcome::Ack`/`Nack`). Consequences: handlers MUST be idempotent (redelivery after a crash or a `Nack` is expected); a `Nack` whose payload was durably published to a configured DLQ counts as processed (acked), while a failed DLQ publish leaves the source unacked; messages that fail to deserialize (poison messages) are parked in the matching handlers' configured DLQs (when any) before ack, not redelivered; a panicking handler counts as `Nack`. Shared consume-loop machinery in `r2e_events::backend`: `WatermarkTracker` (per-partition commit watermark, nack-pinned) and `spawn_completion_forwarder` + `COMPLETION_CHANNEL_CAPACITY`/`COMPLETION_DRAIN_TIMEOUT` (pipelined ack decisions). Kafka additionally tags completions with a per-partition assignment epoch so outcomes from a revoked assignment cannot acknowledge a redelivery. `LocalEventBus` is in-process only — events don't survive a crash (no delivery guarantee across restarts).

**Declarative consumers on controllers** via `#[consumer(bus = "field_name")]` in a `#[routes]` impl block. Consumers run on the controller core (which always implements `ContextConstruct`), so they work regardless of any `#[inject(identity)]` fields. Consumers are registered automatically by `AppBuilder::register_controller`. Since W10 phase 3 controller consumers use the same bean-level transverse machinery: they accept `#[intercept(...)]` (method-level and an impl-level `#[intercept]` on the `#[routes]` block wrapping every `#[scheduled]`/`#[consumer]` method, impl-level outermost), for both fan-out subscribers and responders, with direct in-code calls self-intercepting through the core's decorator slot (filled once by `Controller::fill_decos`). A missing decorator bean is a compile error at `.register_controller`; `#[scheduled]` + `#[consumer]` on one method is also a compile error. See `docs/claude/guards-interceptors.md`.

**Controller `#[post_construct]` lifecycle hooks** (W10 phase 3) — a `#[routes]` impl may declare `#[post_construct]` methods (same signature rules as bean hooks: `&self` only, sync or async, `()` or `Result<(), Box<dyn Error + Send + Sync>>`). They are queued at `register_controller` and awaited at startup **before** consumer registrations begin — later than bean `#[post_construct]` (which runs inside `build_state()`), because cores are built after the graph resolves. An `Err` aborts startup. See `docs/claude/beans-di.md`.

**Declarative consumers on beans** via `#[consumer(bus = "field_name")]` in a `#[bean]` impl block. The `#[bean]` macro generates an `EventSubscriber` impl plus an `after_register` hook (`BeanRegistry::register_event_subscriber`), so `.register::<T>()` alone is enough — `build_state()` queues the subscription and it runs at server startup (`serve` / `build_with_consumers`), same auto-collection as `#[scheduled]` (no explicit `register_subscriber` call; the method was removed). Provided (`.provide(...)`) instances do not auto-subscribe — register the type, or use `add_consumer_registration`.

**Multiple buses** — both controllers and beans can use multiple bus fields of different types. Each `#[consumer(bus = "field")]` references a specific field.

**EventBus↔SSE bridge** — `r2e_events::sse_bridge`. `SseTopic<E>` (r2e-core `sse` module, in the prelude) is a typed broadcast-topic bean over `SseBroadcaster`: `publish(&E)` serializes (JSON by default; `with_serializer` swaps the text format) under the topic's SSE event name (default: short type name of `E`; `with_event_name` to override; `Ok(0)` when no subscribers); `subscribe()` returns an `SseSubscription` ready for `#[sse]` handlers. `SseBridgeExt::bridge_sse::<Bus, E>()` (post-`build_state`, in the prelude) pulls the bus and `SseTopic<E>` beans from the bean context and registers a forwarding consumer at startup — `bus.emit(event)` fans out to SSE with zero liaison code, cross-instance with distributed backends. Manual entry point: `bridge_event_to_sse(&bus, topic)`. The underlying extension hook is `AppBuilder::add_consumer_registration` (same drain as `#[consumer]`; also run by `TestApp::boot` via `BootableApp::into_router_with_consumers`, so consumers and bridges are live in tests).

### IggyEventBus (r2e-events-iggy)

`IggyEventBus` — distributed `EventBus` implementation backed by [Apache Iggy](https://iggy.apache.org/). Publishes events as JSON to Iggy topics; background pollers consume and dispatch to local handlers.

**Setup:**
```rust
let config = IggyConfig::builder()
    .address("127.0.0.1:8090")
    .stream_name("my-app")
    .consumer_group("my-group")
    .build();

let bus = IggyEventBus::builder(config)
    .topic::<UserCreated>("user-created")   // explicit topic name
    .topic::<OrderPlaced>("order-placed")
    .connect()
    .await?;
```

**Key types:**
- `IggyConfig` — connection settings (address, transport, stream name, consumer group, poll interval, auto-create).
- `Transport` — `Tcp` (default) | `Quic` | `Http`.
- `IggyEventBusBuilder` — pre-register topic names, then `.connect().await` to create the bus.

**Behavior:**
- `subscribe<E>()` — registers a local handler; on first subscriber for a type, spawns a background poller that creates/joins an Iggy consumer group.
- `emit()` / `emit_with()` — serializes to JSON, maps `EventMetadata` to Iggy headers (`r2e-event-id`, `r2e-correlation-id`, `r2e-timestamp`, `r2e-h-*`), publishes to Iggy.
- `request()` / `respond()` — point-to-point request-reply over a shared request topic + per-instance reply topic + correlation header (an absent responder surfaces as `RequestTimeout`, not `NoResponder`).
- `shutdown(timeout)` — cancels pollers, drains in-flight handlers, disconnects client.
- Topic names default to sanitized `type_name` (`::` → `.`) unless explicitly registered via builder.

**Feature flag:** `r2e = { features = ["events-iggy"] }` or depend on `r2e-events-iggy` directly.

### KafkaEventBus (r2e-events-kafka)

`KafkaEventBus` — distributed `EventBus` implementation backed by [Apache Kafka](https://kafka.apache.org/) via `rdkafka` (librdkafka binding).

**Setup:**
```rust
let config = KafkaConfig::builder()
    .bootstrap_servers("localhost:9092")
    .group_id("my-group")
    .compression(Compression::Zstd)
    .build();

let bus = KafkaEventBus::builder(config)
    .topic::<UserCreated>("user-created")
    .connect()
    .await?;
```

**Key types:**
- `KafkaConfig` — bootstrap servers, group ID, security protocol, SASL, compression, acks, auto-create, overrides.
- `SecurityProtocol` — `Plaintext` | `Ssl` | `SaslPlaintext` | `SaslSsl`.
- `Compression` — `None` | `Gzip` | `Snappy` | `Lz4` | `Zstd`.
- `Acks` — `Zero` | `One` | `All`.

**Behavior:**
- Single `FutureProducer` shared via `Arc` (thread-safe, connection-pooled).
- One `StreamConsumer` per event type, spawned on first `subscribe()`.
- `partition_key` maps to Kafka message key (determines partition).
- Metadata encoded as Kafka message headers.
- Topic auto-creation via `AdminClient::create_topics()`.
- Shutdown: cancel consumers, `producer.flush(timeout)`.

**Feature flag:** `r2e = { features = ["events-kafka"] }` or depend on `r2e-events-kafka` directly. Build features: `cmake-build` (default), `dynamic-linking`.

### PulsarEventBus (r2e-events-pulsar)

`PulsarEventBus` — distributed `EventBus` implementation backed by [Apache Pulsar](https://pulsar.apache.org/) via the `pulsar` crate.

**Setup:**
```rust
let config = PulsarConfig::builder()
    .service_url("pulsar://localhost:6650")
    .subscription("my-group")
    .build();

let bus = PulsarEventBus::builder(config)
    .topic::<UserCreated>("user-created")
    .connect()
    .await?;
```

**Key types:**
- `PulsarConfig` — service URL, subscription name, subscription type, topic prefix, auth token, batch size, auto-create.
- `SubscriptionType` — `Shared` | `Exclusive` | `Failover` | `KeyShared`.

**Behavior:**
- Producers cached per topic behind `Mutex<HashMap<String, Producer>>`.
- Full topic name: `{topic_prefix}{topic_name}` (default prefix: `persistent://public/default/`).
- `partition_key` maps to Pulsar message key (`KeyShared` routing).
- Metadata maps directly to Pulsar message properties (`HashMap<String, String>`) — zero conversion.
- `consumer.ack()` after successful dispatch; `consumer.nack()` triggers redelivery.

**Feature flag:** `r2e = { features = ["events-pulsar"] }` or depend on `r2e-events-pulsar` directly.

### RabbitMqEventBus (r2e-events-rabbitmq)

`RabbitMqEventBus` — distributed `EventBus` implementation backed by [RabbitMQ](https://www.rabbitmq.com/) via `lapin` (AMQP 0-9-1).

**Setup:**
```rust
let config = RabbitMqConfig::builder()
    .uri("amqp://guest:guest@localhost:5672/%2f")
    .exchange("r2e-events")
    .consumer_group("my-group")
    .build();

let bus = RabbitMqEventBus::builder(config)
    .topic::<UserCreated>("user-created")
    .connect()
    .await?;
```

**Key types:**
- `RabbitMqConfig` — URI, exchange name, consumer group, prefetch count, durable, persistent, dead letter exchange, heartbeat.

**AMQP model mapping:**
- Event bus → Topic exchange (fan-out by routing key).
- Event type → Routing key = topic name.
- Consumer group → Queue named `{consumer_group}.{topic_name}`.
- Competing consumers → Multiple instances consuming the same queue.
- `partition_key` → Stored as AMQP header only (RabbitMQ has no native partitioning).
- Metadata → AMQP headers (`FieldTable` with `AMQPValue::LongString`).

**Behavior:**
- One `Connection` + one `Channel` shared via `Arc`.
- On first `subscribe<E>()`: declare queue, bind to exchange, start `basic_consume()` stream.
- `delivery.ack()` after successful dispatch; `delivery.nack(requeue: true)` on failure.
- Messages are persistent (delivery_mode = 2) when `config.persistent` is true.

**Feature flag:** `r2e = { features = ["events-rabbitmq"] }` or depend on `r2e-events-rabbitmq` directly.

## Scheduling (r2e-scheduler)

Scheduled tasks are auto-discovered on **controllers** (via `register_controller()`) and on **beans** (`#[scheduled]` inside a `#[bean]` impl — `.register::<T>()` alone is enough; `build_state()` collects the tasks. See `beans-di.md` § "`#[scheduled]` on beans"). The scheduler runtime (`r2e-scheduler`) provides the `Scheduler` plugin (unit struct) that installs `CancellationToken`-based lifecycle management.

**Schedule data types** (in `r2e-scheduler`):
- `ScheduleConfig::Interval(duration)` — fixed interval.
- `ScheduleConfig::IntervalWithDelay { interval, initial_delay }` — with initial delay.
- `ScheduleConfig::Cron(expr)` — cron expression (via `cron` crate in the runtime).
- `ScheduleConfig` implements `FromStr` (duration string → `Interval`; whitespace or leading `@` → validated `Cron`) and `FromConfigValue` (string, or integer = seconds) — so `#[config("app.sync.schedule")] schedule: ScheduleConfig` works.
- `ScheduledTaskDef<T>` — a named task definition with schedule and closure. Constructors: `new(name, schedule, state, |state| async)` and stateless `from_fn(name, schedule, || async)`; closures may return `()` or `Result<(), E: Display>` (errors logged).
- `ScheduledResult` — trait for handling `()` or `Result<(), E>` return values.
- `parse_duration("1h30m")` — runtime duration-string parser (same grammar as `#[scheduled(every = "...")]`).

**Declarative scheduling** via `#[scheduled]` attribute. `every` and `initial_delay` accept an integer (seconds) or a duration string (`ms`, `s`, `m`, `h`, `d`, combinable). Cron expressions are validated at compile time.
```rust
#[scheduled(every = 30)]                              // every 30 seconds (integer = seconds)
#[scheduled(every = "5m")]                            // every 5 minutes (duration string)
#[scheduled(every = "1m", initial_delay = "10s")]     // first run after 10s
#[scheduled(cron = "0 */5 * * * *")]                  // cron expression (compile-time validated)
#[scheduled(every = "50ms", overlap = "concurrent")]  // self-overlap policy (default "skip")
```

**Overlap policy (`overlap = "skip" | "concurrent"`, default `skip`; also valid with `cron`).** `skip` (today's behavior) re-arms a job on completion, so a tick that comes due while the previous one is still running is skipped — cadence preserved, never overlaps with itself. `concurrent` re-arms at *fire* time (the next deadline is pushed back before the tick is submitted, and completion does not re-arm), so a slow tick never holds back the next; ticks may pile up. Interval cadence stays anchored; cron recomputes next at fire time. Dynamic tasks: `ScheduledTaskDef::new(..).with_overlap(OverlapPolicy::Concurrent)`.

**Config (`scheduler.*`).** Typed `SchedulerConfig` (`CONFIG_PREFIX = Some("scheduler")`, all keys optional): the standard `scheduler.enabled = false` gate skips starting tasks while the provided beans remain; `scheduler.executor = "shared"` (default — the app-wide `PoolExecutor`) or `"dedicated"` (a private pool sized by `scheduler.max-concurrent` / `queue-capacity` / `shutdown-timeout`, mirroring `executor.*`, with its own graceful drain hook). `PoolExecutor` stays a hard `LateDeps` requirement even in dedicated mode (a type-level requirement cannot be config-conditional). An unrecognized `executor` value panics at boot.

**Runtime control + stats.** `SchedulerHandle` (extract as a handler param, or `SchedulerHandle::channel(token)` to wire it to a manual `start_jobs`) exposes `pause(name).await` / `resume(name).await` / `trigger_now(name).await` (all `-> bool`; `false` = unknown job / no driver / `skip` job already in flight). A paused job advances its cadence silently but never submits; `trigger_now` fires once out of band (allowed even when paused; its OOB tick never re-arms and leaves the schedule untouched). `ScheduledJobInfo` carries live stats the driver updates: `last_run` / `next_run` (`chrono::DateTime<Utc>`), `last_duration`, `run_count`, `panic_count`, `paused` — read via `ScheduledJobRegistry::list_jobs()` / `job(name)`.

**Requires the Executor plugin.** `Scheduler` declares `type LateDeps = (PoolExecutor,)`, so a chain with `.plugin(Scheduler)` but no `PoolExecutor` bean (normally provided by `.plugin(Executor)`) fails at `build_state()` with the standard guided "missing `.provide::<PoolExecutor>()` or `.register::<PoolExecutor>()`" error. `LateDeps` are verified against the final provision list, so the order between `.plugin(Executor)` and `.plugin(Scheduler)` does not matter. The `scheduler` facade feature pulls in `executor`.

**Single-driver model.** All schedules are driven by ONE `rt::spawn`ed driver task (`start_jobs`), not one Tokio task per schedule. The driver owns a min-heap of next-fire deadlines (`ScheduledTask::into_job` → `ScheduledJob`); when the earliest deadline is reached it submits the due tick bodies to the shared `PoolExecutor` and tracks the `JobHandle`s in a `FuturesUnordered`. Under the default `skip` policy a job is re-armed onto the heap only when its own tick completes — so it is either in the heap or in flight, never both; under `concurrent` it is re-armed at fire time and may have several ticks in flight. The driver also accepts runtime `pause`/`resume`/`trigger_now` commands and keeps `ScheduledJobInfo` stats current.

**Pool-tick execution (Quarkus model).** Each scheduled tick runs as a pool job (`executor.submit(...)`). Non-overlap is preserved (`MissedTickBehavior::Skip` semantics — a slow tick blocks only its own schedule), while different jobs still run concurrently (the driver never awaits a tick inline). In-flight ticks drain on shutdown (they are pool jobs covered by `executor.shutdown-timeout` / `PoolExecutor::shutdown_graceful`); the driver breaks on cancellation without aborting them. A panicking tick is contained in the pool job, logged, and its job is re-armed. Scheduled work is globally bounded by `executor.max-concurrent` and appears in `ExecutorMetrics` (running/queued/completed/rejected); when the pool is shut down, the driver stops.

**Registration:** install the `Executor` and `Scheduler` plugins before `build_state()`, then register controllers:
```rust
AppBuilder::new()
    .plugin(Executor)                         // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)                        // install scheduler runtime (provides CancellationToken)
    .build_state()
    .await
    .register_controller::<ScheduledJobs>()   // auto-discovers #[scheduled] methods
    .serve("0.0.0.0:3000")
```

The `Controller` trait's `scheduled_tasks_boxed()` method (auto-generated by `#[routes]`) returns type-erased task definitions; `register_controller()` collects them into the shared `TaskRegistryHandle`. Bean scheduled tasks flow through the same registry: `#[bean]` generates a `ScheduledSource` impl and an `after_register` hook (`BeanRegistry::register_scheduled_source`), and `build_state()` drains those hooks against the resolved graph. `serve()` passes all collected tasks to the scheduler backend, which drives the schedules and submits each tick body to the `PoolExecutor`. On shutdown, the `CancellationToken` is cancelled.

Scheduled tasks run on the controller core, which always implements `ContextConstruct` (identity and request-scoped fields live only on the per-request façade). Controllers can therefore be used for scheduling regardless of any struct-level or param-level `#[inject(identity)]`.

### Dynamic (config-driven) tasks — `AppBuilderSchedulerExt`

For tasks whose set is only known at startup (e.g. one task per configured source), use `schedule_task` / `schedule_tasks` on the post-`build_state()` builder instead of `#[scheduled]`, or the `_with` variants (`schedule_task_with` / `schedule_tasks_with`) whose closure receives the resolved `BeanContext` for pulling task state by type. Same lifecycle as static tasks: started at serve, listed in `ScheduledJobRegistry`, cancelled on shutdown. Must be called before `serve()`. Panics if the `Scheduler` plugin is missing. Full doc: `docs/features/21-dynamic-scheduled-tasks.md`.

```rust
use r2e_scheduler::{AppBuilderSchedulerExt, ScheduledTaskDef};

AppBuilder::new()
    .plugin(Executor)                     // required by Scheduler
    .plugin(Scheduler)
    .provide(sync_service)
    .build_state()
    .await
    .schedule_task_with(|ctx| ScheduledTaskDef::new(
        format!("sync_{}", source.name),
        source.schedule.clone(),          // ScheduleConfig, e.g. from #[config(...)]
        ctx.get::<SyncService>(),         // bean-backed task state
        move |svc| async move { svc.sync().await },   // may return Result
    ))
    .serve("0.0.0.0:3000")
```

### SchedulerHandle

`SchedulerHandle` — extractable Axum handler parameter providing runtime control over the scheduler. Implements `FromRequestParts`. Available when `Scheduler` plugin is installed.

- `cancel()` — cancel the scheduler and all running tasks (triggers the `CancellationToken`).
- `is_cancelled()` — check if the scheduler has been cancelled.
- `token()` — get the underlying `CancellationToken` clone.

```rust
#[get("/scheduler/status")]
async fn status(&self, scheduler: SchedulerHandle) -> Json<bool> {
    Json(scheduler.is_cancelled())
}
```

### ScheduledJobRegistry

`ScheduledJobRegistry` — injectable bean providing runtime introspection of registered scheduled jobs. Provided automatically by the `Scheduler` plugin. Inject via `#[inject]` on controller/bean fields.

- `list_jobs()` — returns `Vec<ScheduledJobInfo>` with `name` and `schedule` (human-readable, e.g., `"every 30s"`, `"cron: 0 */5 * * * *"`).
- `register(info)` — called internally when tasks are started; not typically used directly.

```rust
#[controller(path = "/admin")]
pub struct AdminController {
    #[inject] jobs: ScheduledJobRegistry,
}

#[routes]
impl AdminController {
    #[get("/jobs")]
    async fn list_jobs(&self) -> Json<Vec<ScheduledJobInfo>> {
        Json(self.jobs.list_jobs())
    }
}
```

## Pagination and database transactions

- `Pageable` and `Page<T>` live in `r2e-core` and are always available.
- `r2e-data-sqlx` contains only cancellation-safe managed SQLx transactions.
- `r2e-data-diesel` contains only managed Diesel/r2d2 transactions and a
  blocking-pool `run` helper.
- CRUD models and queries remain application-owned and use SQLx or Diesel
  directly.

## Cache (r2e-cache)

`TtlCache<K, V>` — thread-safe TTL cache backed by `DashMap`. Supports get, insert, remove, clear, evict_expired.

`CacheStore` trait — pluggable async cache backend. Default: `InMemoryStore` (DashMap-backed). Supports get, set, remove, clear, remove_by_prefix. The store is an application **bean** (`Arc<dyn CacheStore>`): provide one with `.provide(InMemoryStore::shared())`. (The old global `set_cache_backend()`/`cache_backend()` singleton was deleted in Phase 6.)

The `Cache` interceptor (in `r2e-utils`) resolves the store bean at controller registration (`DecoratorSpec` — a missing store is a compile error at `register_controller()`). `#[intercept(Cache::ttl(30).group("users"))]` stores in a named group; `#[intercept(CacheInvalidate::group("users"))]` clears by prefix.

## Rate Limiting (r2e-rate-limit)

`RateLimiter<K>` — generic token-bucket rate limiter keyed by arbitrary type. `RateLimitBackend` trait for pluggable backends (default: `InMemoryRateLimiter`). `RateLimitRegistry` — clonable bean; the `RateLimit`/`PreRateLimit` specs pull it once at controller registration into the built guards.

Key kinds: `"global"` (shared bucket), `"user"` (per authenticated user sub), `"ip"` (per X-Forwarded-For).

## OpenAPI (r2e-openapi)

- Generates **OpenAPI 3.1.0** specs. Uses **schemars 1.x** (JSON Schema Draft 2020-12) for schema generation.
- `OpenApiConfig` — configuration for the generated spec (title, version, description). `with_docs_ui(true)` enables the interactive documentation page.
- `OpenApiPlugin` — registers OpenAPI routes. Use `.with(OpenApiPlugin::new(config))` on the builder.
- `SchemaRegistry` — extra schema collection. `register_for::<T: JsonSchema>()` for schemars types, `register(name, value)` for manual schemas. Wire into `OpenApiConfig` via `with_schema::<T>()`, `with_raw_schema(name, json)`, `with_schema_registry(registry)`, `with_schema_override(name, json)`. Precedence: overrides > route schemas > registry > built-in error schemas.
- `SchemaProvider` — trait for types without `JsonSchema` derive; returns `Cow<'static, str>` name + `Value` schema. Use `SchemaRegistry::register_provider::<T>()` to register.
- Route metadata is collected from `Controller::route_metadata()` via `RouteInfo` (in `r2e-core/src/meta.rs`).
- Always serves the spec at `/openapi.json`. When `docs_ui` is enabled, also serves an interactive API documentation UI at `/docs`.
- **Users must add `schemars = "1"` to their Cargo.toml** and derive `JsonSchema` on request/response types. This is required because `schemars_derive` generates code referencing `schemars::` by crate name (same pattern as serde).
- Request body schemas: auto-detected from `Json<T>` params (`application/json`) and `TypedMultipart<T>` params (`multipart/form-data`; schema from the `MultipartSchema` impl generated by `#[derive(FromMultipart)]`, file fields modeled as `type: string, format: binary`). Raw `Multipart` params produce a free-form `multipart/form-data` object body. `Option<Json<T>>` → `required: false`. `RouteInfo.request_body_content_type` carries the media type (`None` = JSON).
- Response schemas: auto-detected from return types (`Json<T>`, `JsonResult<T>`, `Result<Json<T>, _>`). Uses autoref specialization to gracefully skip types missing `JsonSchema`.
- Status codes: smart defaults (GET→200, POST→201, DELETE→204). Override with `#[status(N)]`.
- `#[returns(T)]` — explicit response type for opaque returns (`impl IntoResponse`).
- `#[deprecated]` — standard Rust attribute, reflected in spec.
- Doc comments: first `///` line → `summary`, remaining → `description`.
- 401/403 responses: only emitted when route has auth (`#[roles]`, `#[inject(identity)]`, guards).

## Static File Serving (r2e-static)

`EmbeddedFrontend` — plugin that serves static files embedded in the binary via `rust_embed`, with SPA fallback support. Installs as a fallback handler on the Axum router.

- **Quick start:** `app.with(EmbeddedFrontend::new::<Assets>())` — serves files from a `#[derive(Embed)]` struct with sensible defaults (SPA on, `api/` excluded, `assets/` immutable).
- **Builder API:** `EmbeddedFrontend::builder::<Assets>()` for custom configuration. Builder methods:
  - `spa_fallback(bool)` — enable/disable SPA fallback (default `true`).
  - `fallback_file(impl Into<String>)` — file served for unmatched routes in SPA mode (default `"index.html"`).
  - `exclude_prefix(impl Into<String>)` — add a path prefix to bypass static serving (default `"api/"`). Call multiple times to add more.
  - `clear_excluded_prefixes()` — remove all excluded prefixes including the default.
  - `immutable_prefix(impl Into<Option<String>>)` — prefix for immutable cache headers (default `Some("assets/")`). Pass `None` to disable.
  - `immutable_cache_control(impl Into<String>)` — `Cache-Control` for immutable files (default `"public, max-age=31536000, immutable"`).
  - `default_cache_control(impl Into<String>)` — `Cache-Control` for other files (default `"public, max-age=3600"`).
  - `base_path(impl Into<String>)` — mount under a sub-path (e.g., `"/docs"`); the base path is stripped before file lookup.
  - `.build()` — finalize and return the plugin.
- **FileServer trait** — object-safe abstraction over `rust_embed::Embed`. `EmbedAdapter<E>` wraps any `Embed` type.
- **Handler logic:** check excluded prefixes → exact file match → directory index (`foo/` → `foo/index.html`) → SPA fallback → 404.
- **Cache headers:** files under `immutable_prefix` (default `assets/`) get `Cache-Control: public, max-age=31536000, immutable`. Others get `public, max-age=3600`.
- **ETag:** SHA-256 hash from `rust_embed` metadata, served as `ETag` header.
- **`should_be_last() = true`** — the fallback handler must be installed after all route registrations.
- **Feature flag:** `r2e = { features = ["static"] }` or depend on `r2e-static` directly.

## Testing (r2e-test)

- **App boot (the `@QuarkusTest` path)** — apps declare `impl App for MyApp` once in `app.rs`; `lib.rs` includes it for tests and `r2e::app_main!(MyApp)` includes it in the binary tip crate while generating `main`. `setup()` owns long-lived resources and `build(b, env) -> impl BootableApp` assembles the app; tests boot the real app **by type** instead of re-declaring controllers:
  - `TestApp::boot::<MyApp>().await` — forces the `test` profile (so `load_config()` overlays `application-test.yaml`) and pins a fresh `TestJwt`'s `Arc<JwtClaimsValidator>`/`Arc<JwtValidator>` over the app's own validator.
  - `TestApp::boot_with::<MyApp>(|b| ...).await` — same, plus a builder hook to pin mocks (`b.override_bean(mock)`) and patch config (`b.override_config_value(key, value)`; or `b.override_config(cfg)` for a full in-memory config). Pinned overrides win over the app's later registrations (first-pin semantics: the harness pre-configures the builder *before* `build` runs, so test overrides must beat later registrations).
  - `TestApp::boot_plain::<MyApp>(|b| ...).await` — skips the TestJwt wiring.
  - `#[r2e::test(app = my_app::MyApp)]` — macro form; `app` is the app **TYPE**. Binds test-fn params: `app: TestApp`, `jwt: TestJwt`, `#[inject] bean: T`. Optional `with = |b| ...` and `jwt = false`.
  - **Ordered tests (`@Order`)** — `#[r2e::test(order = <u32>)]` runs tagged tests sequentially in ascending order within the same test **binary** (one file under `tests/`); scope is the binary, never cross-binary/cross-crate. Orders need not be contiguous (10, 20, 30). Works with or without `app = …`; the barrier covers TestApp boot too (no dev-service races). Optional `group = "<name>"` gives independent sequences in one binary — a test waits only on lower orders of its OWN group (default = the unnamed group). Registry filled at binary load via `inventory`; each ordered test awaits (async barrier in `r2e-test`) all lower **registered** orders of its group. Non-ordered tests are untouched and stay parallel (no `--test-threads=1`). **Fail-fast:** a panicking ordered test poisons its group — later tests fail immediately naming the failed predecessor (no deadlock). **Duplicate `order` in a group:** runtime panic naming both tests (macro can't see siblings, so not a compile error). **Watchdog:** a waiting test panics (not hangs) if its group makes no progress for `R2E_TEST_ORDER_TIMEOUT_SECS` (default 60) — typically a filtered-out lower order or `--test-threads` starvation; diagnostic lists pending orders + whether they started. Compile errors: `group` without `order`; `order`/`group` on `#[r2e::main]`. Using `order` requires the `r2e-test` dev-dependency (already present with `app = …`).
  - `app.bean::<T>()` — fetch any bean from the booted app's resolved graph. `app.config()`, `app.test_jwt()` accessors. `.as_user(sub, &roles)` on `TestRequest`/`SessionRequest`/`TestSession` mints a Bearer token from the app's `TestJwt` (the `@TestSecurity` equivalent).
- `TestApp` — wraps a `Router` with an HTTP client for integration testing. Methods: `get`, `post`, `put`, `delete`, `patch`, `request` return `TestRequest` builder. Call `.send().await` to execute. `serve()` spawns a live `TestServer` on a random TCP port (needed for WebSocket/SSE). `from_builder` retains the bean graph (so `bean::<T>()` works); `with_jwt(jwt)` attaches a `TestJwt` to a hand-assembled app.
- `TestRequest` — builder with: `bearer(token)`, `header(name, value)`, `json(body)`, `body(bytes)`, `form(fields)`, `cookie(name, value)`, `query(key, value)`, `queries(pairs)`, `content_type(ct)`, `file(field, name, ct, data)`, `field(name, value)`, `multipart()`.
- `TestResponse` — response wrapper with:
  - **Status assertions:** `assert_ok` (200), `assert_created` (201), `assert_no_content` (204), `assert_bad_request` (400), `assert_unauthorized` (401), `assert_forbidden` (403), `assert_not_found` (404), `assert_conflict` (409), `assert_unprocessable` (422), `assert_too_many_requests` (429), `assert_internal_server_error` (500), `assert_status(code)`. All return `&Self`.
  - **JSON-path assertions:** `assert_json_path(path, expected)`, `assert_json_path_fn(path, predicate)`, `json_path::<T>(path)`.
  - **JSON matching:** `assert_json_contains(expected)` (partial/subset match), `assert_json_path_contains(path, item)`.
  - **JSON shape:** `assert_json_shape(schema)` — structural type validation using exemplar values.
  - **Header assertions:** `assert_header(name, expected)`, `assert_header_exists(name)`, `assert_content_type(expected)`.
  - **Cookie attribute assertions:** `assert_cookie_secure(name)`, `assert_cookie_http_only(name)`, `assert_cookie_same_site(name, expected)`, `assert_cookie_path(name, expected)`.
  - **SSE assertions:** `sse_events()` → `Vec<ParsedSseEvent>`, `assert_sse_event(type, data)`, `assert_sse_data(data)`.
  - **Access:** `json::<T>()`, `json_optional::<T>()`, `text()`, `bytes()`, `content_type()`, `is_json()`, `header(name)`, `cookie(name)`, `cookies()`, `set_cookie(name)` → `Option<SetCookie>`, `set_cookies()` → `Vec<SetCookie>`.
  - **Construction:** `from_parts(status, headers, body)` — for unit-testing response helpers.
- `TestSession` — cookie-persisting session wrapper. Created via `app.session()`. Builder: `with_bearer(token)`, `with_default_header(name, value)`. Cookie management: `set_cookie`, `remove_cookie`, `clear_cookies`, `cookie`. HTTP methods: `get/post/put/patch/delete/request` return `SessionRequest` (same builder API as `TestRequest`). Cookies from `Set-Cookie` responses are auto-captured.
- `TestJwt` — generates JWT tokens for test scenarios with configurable sub/email/roles. `token_builder(sub)` → `TokenBuilder` with `roles`, `email`, `claim`, `expires_in_secs`, `expired`, `issuer`, `audience`, `algorithm`, `without_sub`, `without_claim`. Convenience: `wrong_issuer_token(sub)`, `wrong_audience_token(sub)`, `wrong_algorithm_token(sub)`, `malformed_token()`.
- `TestServer` — spawns a router on a random local TCP port with graceful shutdown on drop. Methods: `addr()`, `url()`, `ws_url()` (feature `ws`), `ws(path)` (feature `ws`).
- `WsTestClient` (feature `ws`) — WebSocket test client. `send_text`, `send_json`, `send_binary`, `close`. `next_text`, `next_json`, `next_binary` (all with configurable timeout, default 5s). `with_timeout(dur)`, `assert_no_message(wait)`.
- `SetCookie` — parsed `Set-Cookie` header with all attributes: `name`, `value`, `path`, `domain`, `max_age`, `expires`, `secure`, `http_only`, `same_site`.
- `FiniteStream<T>` — yields items from a `Vec` then completes. Use for testing SSE endpoints backed by infinite broadcast streams.
- `ParsedSseEvent` — parsed SSE event with `event: Option<String>` and `data: String`.
- `json_contains(actual, expected)` — recursive subset matching function (exported for custom assertions).
- **Dev services (`r2e-devservices`)** — containerized infra for tests via testcontainers. `DevPostgres` / `DevRedis` (features `postgres`/`redis`): `shared().await` = one stable container per workspace session, reused across test binaries. A shared `testcontainers/ryuk:0.14.0` instance keeps one TCP lease per process and force-removes labelled services after the final process disconnects (10-second grace by default), then auto-removes itself. `start()` = isolated and handle-scoped, with the same Ryuk session as a crash/`SIGKILL` fallback; `start_with_tag(tag)` selects a custom image tag (defaults pinned to `postgres:16-alpine`/`redis:7-alpine` — the modules' own defaults are pre-arm64). `R2E_DEVSERVICES_KEEP=1` disables the reaper for inspection; Ryuk socket/timeout/session/privileged overrides are documented in `r2e-devservices/README.md`. Wire in via `b.override_config_value("app.database.url", pg.url())` in the boot hook. Docker smoke tests: `cargo test -p r2e-devservices --features postgres,redis --test dev_services -- --ignored`.
