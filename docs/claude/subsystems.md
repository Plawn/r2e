# R2E Subsystems Reference

## AppBuilder (r2e-core)

Central orchestrator for assembling an R2E application. Two phases: pre-state and post-state.

```rust
AppBuilder::new()
    // ── Pre-state phase ──
    .plugin(Scheduler)                     // scheduler runtime - MUST be before build_state()
    .load_config::<RootConfig>()             // load yaml + env, construct typed config, auto-register children
    // or: .with_config(config)            // provide a pre-loaded R2eConfig (no child auto-registration)
    .provide(services.pool.clone())        // provide beans
    .with_producer::<CreatePool>()         // async producer (registers SqlitePool)
    .with_async_bean::<MyAsyncService>()   // async bean constructor
    .with_bean::<UserService>()            // sync bean (unchanged)
    // config sections are auto-registered as beans by load_config (inject with #[inject])
    .build_state::<Services, _, _>()       // resolve bean graph (async — .await required)
    .await
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
    .on_stop(|state| async move { state.pool.close().await; })
    .register_controller::<UserController>()
    .register_controller::<AccountController>()
    .register_controller::<ScheduledJobs>() // auto-discovers #[scheduled] methods
    .register_subscriber::<NotificationService>() // bean event subscribers
    .build()                               // → axum::Router
    // or .serve("0.0.0.0:3000").await     // build + listen + graceful shutdown
    // or .serve_auto().await              // reads server.host / server.port from config (defaults: 0.0.0.0:3000)
```

**Lifecycle hooks** (post-state):
- `on_start(|state| async move { Ok(()) })` — runs before the server starts listening. Receives state, returns `Result`.
- `on_stop(|state| async move { })` — runs after graceful shutdown. Receives state, returns `()`.

`build()` returns an `axum::Router`. `serve(addr)` builds, runs startup hooks, registers event consumers, starts scheduled tasks, starts listening, waits for shutdown signal (Ctrl-C / SIGTERM), stops the scheduler, then runs shutdown hooks. `serve_auto()` does the same but reads address from config keys `server.host` (String, default `"0.0.0.0"`) and `server.port` (u16, default `3000`).

`.shutdown_grace_period(Duration)` — optional maximum time for shutdown hooks to complete before force-exiting the process. Without it, the process waits indefinitely.

`.r2e_config()` — returns `Option<&R2eConfig>`, available after `load_config()` or `with_config()`. Used by `Tracing::from_config()` to read tracing settings from YAML.

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

## StatefulConstruct (r2e-core)

`StatefulConstruct<S>` trait allows constructing a controller from state alone (no HTTP context). Auto-generated by `#[derive(Controller)]` when the struct has no `#[inject(identity)]` struct fields. Required for:
- Consumer methods (`#[consumer]`) — event handlers that run outside HTTP requests
- Scheduled methods (`#[scheduled]`) — background tasks

Controllers with `#[inject(identity)]` struct fields do NOT get this impl. Attempting to use them in consumer/scheduled context produces a compile error with a diagnostic message via `#[diagnostic::on_unimplemented]`. Controllers using param-level `#[inject(identity)]` only retain `StatefulConstruct` — this is the key advantage of the mixed controller pattern.

## Configuration (r2e-core)

See [configuration.md](./configuration.md) for the full reference.

**AppBuilder integration** (pre-state methods):
- `load_config::<C>()` (recommended) — load YAML + env, construct typed config (`C: ConfigProperties`), **auto-register all nested `#[config(section)]` children as beans**, provide both `C` and `R2eConfig` in the type list. Use `load_config::<()>()` for raw only.
- `with_config(config)` — provide a pre-loaded `R2eConfig` (tests, hot-reload). Does not auto-register typed config children.

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
- `EventBusError` — `Serialization`, `Connection`, `Shutdown`, `Other`.
- `Event` trait — opt-in trait with `fn topic() -> &'static str` for distributed backends.

**EventBus trait methods:**
- `bus.subscribe(|envelope: EventEnvelope<MyEvent>| async { HandlerResult::Ack })` → `Result<SubscriptionHandle, EventBusError>`. Requires `E: DeserializeOwned`.
- `bus.emit(event)` → `Result<(), EventBusError>`. Fire-and-forget. Requires `E: Serialize`.
- `bus.emit_with(event, metadata)` → `Result<(), EventBusError>`. Emit with explicit metadata.
- `bus.emit_and_wait(event)` → `Result<(), EventBusError>`. Waits for all handlers.
- `bus.emit_and_wait_with(event, metadata)` → `Result<(), EventBusError>`. Waits with explicit metadata.
- `bus.shutdown(timeout)` → `Result<(), EventBusError>`. Graceful shutdown: rejects new emits, waits for in-flight handlers.
- `bus.clear()` — remove all handlers.

Event types must derive `Serialize + Deserialize` (required by the trait for backend compatibility; `LocalEventBus` never actually serializes — zero overhead).

Distributed backends (Kafka, Pulsar, RabbitMQ, Iggy) implement the `EventBus` trait. Shared backend utilities are in `r2e_events::backend` — `TopicRegistry`, `BackendState`, `encode_metadata`/`decode_metadata`.

**Declarative consumers on controllers** via `#[consumer(bus = "field_name")]` in a `#[routes]` impl block. The controller must not have `#[inject(identity)]` struct fields (requires `StatefulConstruct`). Consumers are registered automatically by `AppBuilder::register_controller`.

**Declarative consumers on beans** via `#[consumer(bus = "field_name")]` in a `#[bean]` impl block. The `#[bean]` macro generates an `EventSubscriber` impl. Register via `AppBuilder::register_subscriber::<T>()`.

**Multiple buses** — both controllers and beans can use multiple bus fields of different types. Each `#[consumer(bus = "field")]` references a specific field.

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
- `emit_and_wait()` — publishes to Iggy AND waits for **local** handlers. Cannot wait for remote consumers (documented limitation).
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

Scheduled tasks are auto-discovered via `register_controller()`, following the same pattern as event consumers. The scheduler runtime (`r2e-scheduler`) provides the `Scheduler` plugin (unit struct) that installs `CancellationToken`-based lifecycle management.

**Schedule data types** (in `r2e-core::scheduling`, zero new deps):
- `ScheduleConfig::Interval(duration)` — fixed interval.
- `ScheduleConfig::IntervalWithDelay { interval, initial_delay }` — with initial delay.
- `ScheduleConfig::Cron(expr)` — cron expression (via `cron` crate in the runtime).
- `ScheduledTaskDef<T>` — a named task definition with schedule and closure.
- `ScheduledResult` — trait for handling `()` or `Result<(), E>` return values.

**Declarative scheduling** via `#[scheduled]` attribute. `every` and `initial_delay` accept an integer (seconds) or a duration string (`ms`, `s`, `m`, `h`, `d`, combinable). Cron expressions are validated at compile time.
```rust
#[scheduled(every = 30)]                              // every 30 seconds (integer = seconds)
#[scheduled(every = "5m")]                            // every 5 minutes (duration string)
#[scheduled(every = "1m", initial_delay = "10s")]     // first run after 10s
#[scheduled(cron = "0 */5 * * * *")]                  // cron expression (compile-time validated)
```

**Registration:** install the `Scheduler` plugin before `build_state()`, then register controllers:
```rust
AppBuilder::new()
    .plugin(Scheduler)                        // install scheduler runtime (provides CancellationToken)
    .build_state::<Services, _, _>()
    .await
    .register_controller::<ScheduledJobs>()   // auto-discovers #[scheduled] methods
    .serve("0.0.0.0:3000")
```

The `Controller` trait's `scheduled_tasks()` method (auto-generated by `#[routes]`) returns `Vec<ScheduledTaskDef<T>>`. `register_controller()` collects these. `serve()` passes them to the scheduler backend, which spawns Tokio tasks. On shutdown, the `CancellationToken` is cancelled.

Controllers with `#[inject(identity)]` struct fields cannot be used for scheduling (no `StatefulConstruct` impl). Controllers using param-level `#[inject(identity)]` only retain `StatefulConstruct` and can be used for scheduling.

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
#[derive(Controller)]
#[controller(path = "/admin", state = Services)]
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

## Data (r2e-data)

- `Entity` trait — maps a Rust struct to a SQL table (table name, column list).
- `QueryBuilder` — fluent SQL query builder (`where_eq`, `where_like`, `order_by`, `limit`, `offset`).
- `Repository` trait — async CRUD interface (`find_by_id`, `find_all`, `create`, `update`, `delete`).
- `SqlxRepository` — SQLx-backed implementation of `Repository`.
- `Pageable` — pagination parameters extracted from query string (page, size, sort).
- `Page<T>` — paginated response wrapper (content, total_elements, total_pages, page, size).
- `DataError` — data-layer error type.

## Cache (r2e-cache)

`TtlCache<K, V>` — thread-safe TTL cache backed by `DashMap`. Supports get, insert, remove, clear, evict_expired.

`CacheStore` trait — pluggable async cache backend. Default: `InMemoryStore` (DashMap-backed). Supports get, set, remove, clear, remove_by_prefix. Global singleton via `set_cache_backend()` / `cache_backend()`.

The `Cache` interceptor (in `r2e-utils`) uses the global `CacheStore` backend. `#[intercept(Cache::ttl(30).group("users"))]` stores in a named group; `#[intercept(CacheInvalidate::group("users"))]` clears by prefix.

## Rate Limiting (r2e-rate-limit)

`RateLimiter<K>` — generic token-bucket rate limiter keyed by arbitrary type. `RateLimitBackend` trait for pluggable backends (default: `InMemoryRateLimiter`). `RateLimitRegistry` — clonable handle stored in app state, used by the generated `RateLimitGuard`.

Key kinds: `"global"` (shared bucket), `"user"` (per authenticated user sub), `"ip"` (per X-Forwarded-For).

## OpenAPI (r2e-openapi)

- Generates **OpenAPI 3.1.0** specs. Uses **schemars 1.x** (JSON Schema Draft 2020-12) for schema generation.
- `OpenApiConfig` — configuration for the generated spec (title, version, description). `with_docs_ui(true)` enables the interactive documentation page.
- `OpenApiPlugin` — registers OpenAPI routes. Use `.with(OpenApiPlugin::new(config))` on the builder.
- `SchemaRegistry` / `SchemaProvider` — JSON Schema collection for request/response types.
- Route metadata is collected from `Controller::route_metadata()` via `RouteInfo` (in `r2e-core/src/meta.rs`).
- Always serves the spec at `/openapi.json`. When `docs_ui` is enabled, also serves an interactive API documentation UI at `/docs`.
- **Users must add `schemars = "1"` to their Cargo.toml** and derive `JsonSchema` on request/response types. This is required because `schemars_derive` generates code referencing `schemars::` by crate name (same pattern as serde).
- Request body schemas: auto-detected from `Json<T>` params. `Option<Json<T>>` → `required: false`.
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

- `TestApp` — wraps an `axum::Router` with an HTTP client for integration testing. Methods: `get`, `post`, `put`, `delete`, `patch`, `request` return `TestRequest` builder. Call `.send().await` to execute.
- `TestRequest` — builder with: `bearer(token)`, `header(name, value)`, `json(body)`, `body(bytes)`, `form(fields)`, `cookie(name, value)`, `query(key, value)`, `queries(pairs)`.
- `TestResponse` — response wrapper with:
  - **Status assertions:** `assert_ok` (200), `assert_created` (201), `assert_no_content` (204), `assert_bad_request` (400), `assert_unauthorized` (401), `assert_forbidden` (403), `assert_not_found` (404), `assert_conflict` (409), `assert_unprocessable` (422), `assert_too_many_requests` (429), `assert_internal_server_error` (500), `assert_status(code)`. All return `&Self`.
  - **JSON-path assertions:** `assert_json_path(path, expected)`, `assert_json_path_fn(path, predicate)`, `json_path::<T>(path)`.
  - **JSON matching:** `assert_json_contains(expected)` (partial/subset match), `assert_json_path_contains(path, item)`.
  - **JSON shape:** `assert_json_shape(schema)` — structural type validation using exemplar values.
  - **Header assertions:** `assert_header(name, expected)`, `assert_header_exists(name)`.
  - **Access:** `json::<T>()`, `text()`, `header(name)`, `cookie(name)`, `cookies()`.
- `TestSession` — cookie-persisting session wrapper. Created via `app.session()`. Builder: `with_bearer(token)`, `with_default_header(name, value)`. Cookie management: `set_cookie`, `remove_cookie`, `clear_cookies`, `cookie`. HTTP methods: `get/post/put/patch/delete/request` return `SessionRequest` (same builder API as `TestRequest`). Cookies from `Set-Cookie` responses are auto-captured.
- `TestJwt` — generates valid JWT tokens for test scenarios with configurable sub/email/roles. `token_builder(sub)` → `TokenBuilder` with `roles`, `email`, `claim`, `expires_in_secs`, `expired`. `expired()` sets `exp` to 60 seconds in the past.
- `#[derive(TestState)]` — generates `FromRef` impls for test state structs (eliminates boilerplate). Supports `#[test_state(skip)]`.
- `json_contains(actual, expected)` — recursive subset matching function (exported for custom assertions).
