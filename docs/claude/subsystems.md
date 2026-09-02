# R2E Subsystems Reference

## AppBuilder (r2e-core)

Central orchestrator for assembling an R2E application. Two phases: **builder**
(everything before `build_state()`) and **app** (after it). There is exactly one
plugin kind and one install call — `.plugin(..)`, always in the builder phase.

```rust
AppBuilder::new()
    // ── Builder phase (before build_state) ──
    .plugin(Executor)                      // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)                     // scheduler runtime
    .plugin(Health)                        // /health → 200 "OK"
    .plugin(Cors::permissive())            // or Cors::custom(layer)
    .plugin(HttpTrace::new())              // per-request span + summary line (trace.* keys)
    // subscriber: installed by the entry point; .plugin(Tracing) only if you opted it out
    // or: .plugin(Tracing::configured(config))     // with TracingConfig (format, ansi, etc.)
    // or: .plugin(Tracing::from_config(&r2e_config)) // from YAML tracing.* keys
    .plugin(ErrorHandling)                 // catch panics → JSON 500
    .plugin(DevReload)                     // /__r2e_dev/* endpoints
    .plugin(OpenApiPlugin::new(openapi_cfg)) // /openapi.json (+ /docs if docs_ui enabled)
    .load_config::<RootConfig>()             // load yaml + env, construct typed config, auto-register children (sole config entry)
    // test harness only: .override_config(cfg) BEFORE load_config stashes an in-memory R2eConfig it uses instead of disk
    .provide(services.pool.clone())        // provide beans
    .register::<CreatePool>()              // async producer (registers SqlitePool)
    .register::<MyAsyncService>()          // async bean constructor
    .register::<UserService>()             // sync bean — one unified register()
    // ── Conditional Self->Self assembly (layers, provision list P unchanged) ──
    .when(dev_mode, |b| b.on_start(|_| async { Ok(()) }))   // runtime bool
    // NOTE: `.when()` cannot wrap `.plugin()` (it would change the type-level
    // provision list). Gate a plugin with `<prefix>.enabled = false` instead.
    // For conditional *bean* presence: register a `#[producer] -> Option<T>`
    // (slot always in P; producer returns Some/None). config sections are
    // auto-registered as beans by load_config (inject with #[inject]).
    .build_state()                         // resolve bean graph → inferred HList state (async, no type args)
    .await                                 // .try_build_state().await is the non-panicking variant
    // ── App phase (after build_state) ──
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

**Lifecycle hooks** (app phase, after `build_state()`):
- `on_start(|state| async move { Ok(()) })` — runs before the server starts listening. Receives state, returns `Result`.
- `on_stop(|state| async move { })` — runs after graceful shutdown. Receives state, returns `()`.

`build()` returns a `Router` (from `r2e::http`). `serve(addr)` builds, runs startup hooks, registers event consumers, starts scheduled tasks, starts listening, waits for shutdown signal (Ctrl-C / SIGTERM), stops the scheduler, then runs shutdown hooks. `serve_auto()` does the same but reads address from config keys `server.host` (String, default `"0.0.0.0"`) and `server.port` (u16, default `3000`).

`.shutdown_grace_period(Duration)` — optional maximum time for **each tracked handle** (`spawn_service`, `ServeContext::track`, gRPC/QUIC drains) to finish after the HTTP drain. Without it, shutdown waits indefinitely. It does NOT bound the HTTP drain (that is `.drain_timeout(Duration)`) nor the `on_stop` hooks, which always run.

`.drain_timeout(Duration)` — maximum time for the HTTP drain itself (in-flight requests finishing after the listener stopped accepting), measured from cancellation. **Default 30s** (`runtime::drain::DEFAULT_DRAIN_TIMEOUT`); the `server.drain-timeout` config key sets it without code, and the builder call wins over the key. `.drain_timeout_unbounded()` is the only way back to the plain-axum unbounded drain. On overflow the remaining connections are abandoned with a `warn!` and shutdown continues to the tracked-handle join and the `on_stop` hooks. Applied per worker under sharded serving. See `docs/features/22-serve-lifecycle.md`.

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

**Plugin integration** (subscriber only — the per-request span is `HttpTrace`):
- `Tracing` (unit struct) — uses defaults
- `Tracing::configured(TracingConfig)` → `ConfiguredTracing` — uses explicit config
- `Tracing::from_config(&R2eConfig)` → `ConfiguredTracing` — reads `tracing.*` keys
- `init_tracing_with_config(&TracingConfig)` — low-level function (idempotent)

**Who installs the subscriber (a one-shot process global — first install wins):**
- The **entry point** (`launch_with` when `LaunchOptions::tracing`, i.e.
  `app_main!` / `launch!` / `#[r2e::main]`) calls `init_tracing_from_config()`
  right after `App::setup`: it loads `application.yaml` itself and installs the
  app's `tracing:` section. So `format: json` applies from the first log line
  with no plugin involved; an unreadable file or section falls back to
  `TracingConfig::default()` and warns through the subscriber it just installed.
- A `Tracing` / `ConfiguredTracing` plugin installs in `Plugin::setup`, i.e.
  inside `App::build` — **after** the entry point. It therefore loses the race
  unless the app opts the entry point out with `app_main!(MyApp, tracing =
  false)`. Losing is silent when both sides resolve to the same `TracingConfig`
  (the usual case: both read `tracing.*`) and warns otherwise.
- `init_tracing()` installs the **built-in defaults**, ignoring the app's
  section. It is what `#[r2e::test]` and the bare `Tracing` plugin call, and it
  stays silent when it loses.
- `try_init_tracing_with_config(&cfg) -> Result<(), SubscriberAlreadyInstalled>`
  is the reporting form; `SubscriberAlreadyInstalled::changes_output(&cfg)` says
  whether losing actually changed the output (an unknown winner always counts as
  different), and `warn_if_output_differs(&lost, &cfg)` is the warning both the
  entry point and `ConfiguredTracing` emit. The winning config is recorded in a
  process-global `OnceLock`, which is what makes "same config, stay quiet"
  possible.
  Tests: `r2e-core/tests/tracing_install/` (own target — one-shot global).

**In ObservabilityConfig:**
`ObservabilityConfig` embeds `tracing: TracingConfig`. The `from_r2e_config()` loader reads from `observability.tracing.*`. Convenience method: `.with_log_format(LogFormat)` delegates to the embedded `TracingConfig`.

## HttpTrace (r2e-core)

The per-request HTTP layer: **one** span + **one** summary event per request.
Plugin/builder/config in `src/builtins/http_trace.rs`, layer in
`src/runtime/http_trace.rs`. `Tracing` says where logs go; `HttpTrace` says what
each request logs.

- Span: `request`, INFO, target `r2e::http`, **entered for the whole handler
  future** (so handler logs inherit `route`/`request_id`). `DefaultRequestSpan`
  fields: `method`, `route` (the bounded `MatchedPath` template — never the raw
  path), `request_id`, `headers` (the configured `capture-headers`, recorded as
  one `name=value name=value` field: `tracing` field names are `&'static`
  callsite metadata, so per-header field names are impossible; the OTel shape
  does emit real per-header attributes), `status`.
- Summary event inside the span: `request completed` at INFO, ERROR at 5xx, with
  `status` + `latency_ms` (measured to the response **head**, not the streamed
  body — that is why the Prometheus layer keeps its own timer).
- `record-path` / `record-query` add the raw path/query to the **event only**,
  never to the span (a span decorates every handler log line; paths carry ids).
- Request id: reuses an existing `RequestId` extension, else an inbound
  `x-request-id`, else mints a UUID v4 (`fresh_request_id`, shared with
  `RequestIdPlugin`); publishes it as both the extension and the request header
  and echoes it on the response. Installing `RequestIdPlugin` too, in either
  order, is harmless.
- Exclusions: prefix match on the raw path **or** the route label, via the
  shared `r2e_core::http::labels::path_excluded` (same helper as
  `prometheus.exclude-paths`). An excluded request gets no span, no event, no
  request id — untouched pass-through. Default `["/health", "/metrics"]`.
- Config: `HttpTraceConfig` (`ConfigProperties`, kebab keys), section `trace`,
  gate `trace.enabled` (false = no layer at all). Precedence per knob:
  **explicit builder setting > app `trace:` section > `HttpTrace::preset(cfg)` >
  built-in default**. `capture-headers` are parsed into `HeaderName`s at build —
  an invalid name is a boot error.
- `MakeRequestSpan` is the span-shape seam (`make_span` + `on_response(&Span,
  &RequestOutcome)`, the latter defaulting to the standard field recording +
  summary event). `HttpTraceBuilder::make_span(..)` swaps it; that is exactly
  how `Observability` reuses this layer.
- Tests: `r2e-core/tests/http/http_trace.rs`.

## Observability (r2e-observability)

`Observability` is the OpenTelemetry superset of `Tracing` **and** `HttpTrace`.
It exports with OTLP/HTTP, installs W3C propagation, correlates logs with
`trace_id`/`span_id`, and installs `r2e-core`'s `HttpTraceLayer` with the
`OtelRequestSpan` shape (`src/span.rs`) — so there is exactly **one** span per
request. Do not install it alongside `Tracing` (it owns the subscriber) or
alongside `HttpTrace` (two layers = two spans); it reads the same `trace:`
section either way, with its own `capture_headers` filling the preset slot.

- `Observability::new(config)` — explicit OTLP configuration.
- `Observability::from_config(&r2e_config, service_name)` — reads
  `observability.*`, including `otlp-protocol`.
- `Observability::from_env(service_name)` — reads standard `OTEL_*` variables;
  without an endpoint it installs standard tracing without an exporter.
- Honest OTLP defaults: HTTP/protobuf and
  `http://localhost:4318/v1/traces`; pathless HTTP(S) endpoints receive the
  standard traces path. A requested gRPC protocol warns and uses HTTP.
- `traced_reqwest_client(client)` wraps a `reqwest::Client` so every request
  runs in an `otel.kind = "client"` span (semconv HTTP-client attrs, name
  `HTTP {method}`) and the injected `traceparent` carries that client span's
  id — what Tempo/Jaeger need to draw a service-graph edge. Built on
  `reqwest-tracing` (feature `opentelemetry_0_32`: **must** track the
  workspace `opentelemetry`/`tracing-opentelemetry` bump, otherwise the
  global propagator is a different crate and injection becomes a silent
  no-op). `R2eSpanBackend` is the span backend; `OtelName`/`OtelPathNames`/
  `DisableOtelPropagation` are re-exported per-request extensions.
  `inject_current_context(headers)` is the lower-level SDK chokepoint helper:
  headers only, no client span, hence no service-graph edge.

SQLx 0.9 already emits timed `sqlx::query` tracing events (including
`elapsed_secs`, row counts, and slow-query fields) inside the current request
span. Prefer measuring those events in real traces before adding one OTel span
per query; configure SQLx statement/slow-statement levels through its
`ConnectOptions` when needed.

## ContextConstruct (r2e-core)

`ContextConstruct` trait allows constructing a controller core from the resolved bean graph alone (no HTTP context): `fn from_context(ctx: &BeanContext) -> Self` resolves each `#[inject]` field **by type** (`ctx.get::<T>()`) and each `#[config]` field from `R2eConfig`. Auto-generated by `#[controller]` for every controller core (always — the generated core holds `#[inject]` app fields, `#[config]` fields, and a hidden `DecoSlot` for `#[scheduled]`/`#[consumer]`-method interceptor sets; identity and request-scoped fields are stripped into the per-request façade). It replaces the removed `StatefulConstruct<S>` (which resolved from a hand-written state struct by field name). Required for:
- Consumer methods (`#[consumer]`) — event handlers that run outside HTTP requests
- Scheduled methods (`#[scheduled]`) — background tasks

Because the core never holds identity fields, `ContextConstruct` is available even on controllers that use struct-level or param-level `#[inject(identity)]` — consumers and scheduled tasks operate on the core.

## Configuration (r2e-core)

See [configuration.md](./configuration.md) for the full reference.

**AppBuilder integration** (builder-phase methods, before `build_state()`):
- `load_config::<C>()` (the sole config registration point) — load YAML + env, construct typed config (`C: ConfigProperties`), **auto-register all nested `#[config(section)]` children as beans**, provide both `C` and `R2eConfig` in the type list. Use `load_config::<()>()` for raw only.
- `override_config(config)` — stash a pre-loaded/in-memory `R2eConfig` that the next `load_config` consumes instead of reading disk (test-harness primitive, not dev-reload plumbing — under `dev-reload`, `build()` re-runs per patch and its `load_config` re-reads `application.yaml`). Not a registration point on its own; `load_config` must still be called (else `build_state` panics).

Config sections registered via `load_config` are available as bean dependencies and for `#[inject]` in controllers.

## Security (r2e-security)

- `AuthenticatedUser` implements `FromRequestParts` and `Identity` — extracts Bearer token, validates via `JwtValidator`, returns user with sub/email/roles/claims.
- Claims are **typed**: `r2e_core::StandardClaims` (lives in `r2e-core` because `Identity::claims()` is declared there; re-exported by `r2e-security` and the prelude) — `sub`, `email`, `exp/iat/nbf`, `iss`, `aud: Option<Audience>`, `scope`, `roles`, Keycloak `realm_access`/`resource_access`, plus `#[serde(flatten)] extra` for everything else (`get(&str)` reads `extra` only). `Identity::claims()`, `GuardContext::identity_claims()`, `RoleExtractor::extract_roles`, `IdentityBuilder::build`, `JwtClaimsValidator::validate` and `extract_jwt_claims` all use it; `serde_json::Value` survives only as the `validate_as::<Value>` / `JwtClaimSet` escape hatch. `sub` defaults to `""` and `JwtClaimSet::subject()` maps that to `None`, so a missing subject is still the precise `Token has no 'sub'` rejection. gRPC uses the same type: `JwtClaimsValidatorLike::validate -> StandardClaims`, implemented by `JwtClaimsValidator` under `r2e-security`'s `grpc` feature (`r2e/grpc` enables it via `r2e-security?/grpc`).
- `JwtValidator` supports both static keys (testing) and JWKS endpoint (production) via `JwksCache`.
- `SecurityConfig` — configuration for JWT validation (issuer, audience, JWKS URL, static keys).
- `#[roles("admin")]` attribute generates a guard that checks identity roles via the `Identity` trait and returns 403 if missing.
- Role extraction is trait-based (`RoleExtractor`) to support multiple OIDC providers; default (`DefaultRoleExtractor`) checks top-level `roles` and Keycloak's `realm_access.roles`; extractors take `&StandardClaims` (field reads, no path walking).

## Embedded OIDC (r2e-oidc)

`OidcServer` — embedded OAuth 2.0 access-token issuer plugin. Generates RSA-2048 keys, issues JWT tokens, and exposes `/oauth/authorize`, `/oauth/token`, discovery, JWKS and `/userinfo`. `ClientRegistry::add_public_client` enables local browser login plus Authorization Code with mandatory PKCE S256: exact redirect allowlist, one-time expiring codes bound to client/redirect/resource/challenge, public token endpoint auth (`none`). It does not issue ID tokens or provide federation/SSO. Implements `Plugin` and provides `Arc<JwtClaimsValidator>` to the bean graph.

`OidcRuntime` — pre-built OIDC runtime (`Clone`). Created via `OidcServer::build()`. Holds all expensive state (`Arc`-wrapped RSA keys, user store, client registry). Reusable across hot-reload cycles — only re-registers routes without regenerating keys. Also implements `Plugin`.

Two usage patterns:
- **Simple:** `AppBuilder::new().plugin(OidcServer::new().with_user_store(users))` — generates keys on each install. Works without hot-reload.
- **Hot-reload:** `let oidc = OidcServer::build();` in `setup()`, then `.plugin(oidc.clone())` in `main(env)`. Tokens survive hot-patches.

Key types: `InMemoryUserStore`, `OidcUser`, `UserStore` trait, `ClientRegistry`, `OidcServerConfig`.

## Events (r2e-events)

`EventBus` — pluggable event bus **trait**. `LocalEventBus` — default in-process implementation. Events are dispatched by `TypeId`. Subscribers receive `EventEnvelope<E>` containing `Arc<E>` + `EventMetadata`.

**Core types:**
- `EventEnvelope<E>` — wraps `event: Arc<E>` + `metadata: Arc<EventMetadata>` (the metadata `Arc` is shared across all handlers of one emit — pointer bump per handler, same `event_id` seen by all; deref for reads, clone a field to own it).
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

**`#[pre_destroy]` disposal hooks** (W5) — the `@PreDestroy` counterpart of `#[post_construct]`, on `#[bean]` impls AND `#[routes]` controller impls (same signature rules). Runs during graceful shutdown, in the async shutdown phase: controller hooks first, then bean hooks, each in **reverse registration order**. An `Err` is logged and swallowed (never aborts shutdown); a pinned `override_bean` skips the hook. `#[bean]` generates `impl PreDestroy` + `register_pre_destroy`; a controller core (not `Clone`) uses the `Controller::pre_destroy(core)` override run from its `Arc`. In tests it fires on `TestApp::shutdown().await` (the production shutdown sequence — see the Testing section); the router-only `build_with_consumers` path has no shutdown, so nothing fires there. See `docs/claude/beans-di.md`.

**Declarative consumers on beans** via `#[consumer(bus = "field_name")]` in a `#[bean]` impl block. The `#[bean]` macro generates an `EventSubscriber` impl plus an `after_register` hook (`BeanRegistry::register_event_subscriber`), so `.register::<T>()` alone is enough — `build_state()` queues the subscription and it runs at server startup (`serve` / `build_with_consumers`), same auto-collection as `#[scheduled]` (no explicit `register_subscriber` call; the method was removed). Provided (`.provide(...)`) instances do not auto-subscribe — register the type, or use `add_consumer_registration`.

**Multiple buses** — both controllers and beans can use multiple bus fields of different types. Each `#[consumer(bus = "field")]` references a specific field.

**EventBus↔SSE bridge** — `r2e_events::sse_bridge`. `SseTopic<E>` (r2e-core `sse` module, in the prelude) is a typed broadcast-topic bean over `SseBroadcaster`: `publish(&E)` serializes (JSON by default; `with_serializer` swaps the text format) under the topic's SSE event name (default: short type name of `E`; `with_event_name` to override; `Ok(0)` when no subscribers); `subscribe()` returns an `SseSubscription` ready for `#[sse]` handlers. `SseBridgeExt::bridge_sse::<Bus, E>()` (post-`build_state`, in the prelude) pulls the bus and `SseTopic<E>` beans from the bean context and registers a forwarding consumer at startup — `bus.emit(event)` fans out to SSE with zero liaison code, cross-instance with distributed backends. Manual entry point: `bridge_event_to_sse(&bus, topic)`. The underlying extension hook is `AppBuilder::add_consumer_registration` (same drain as `#[consumer]`; also run by `TestApp::boot` via `BootableApp::start_in_process`, so consumers and bridges are live in tests).

**`#[sse]` streams terminate at shutdown, by default** — `r2e_core::web::sse::{shutdown_token_of, until_shutdown}` (both `#[doc(hidden)]`, the generated-code surface). `generate_sse_closure` resolves the app's `rt::ShutdownToken` from the bean context **once at registration** and passes it to the invocation function as a trailing `Option<ShutdownToken>` prefix param; `until_shutdown` wraps the handler's stream in `futures_util::StreamExt::take_until` over `cancelled_owned()`, or over `pending()` when the bean is absent (a router built with no graph). `None` is therefore the exact previous behaviour, and the cost per request is zero — no extraction, no user-visible field. Rationale: an SSE response is an in-flight HTTP request, so an idle subscriber holds the step-3 drain for the whole `drain_timeout` (30s default). See `docs/features/22-serve-lifecycle.md` § "SSE streams end on it, by default".

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

Scheduled tasks are auto-discovered on **controllers** (via `register_controller()`) and on **beans** (`#[scheduled]` inside a `#[bean]` impl — `.register::<T>()` alone is enough; `build_state()` collects the tasks. See `beans-di.md` § "`#[scheduled]` on beans"). The scheduler runtime (`r2e-scheduler`) provides the `Scheduler` plugin (unit struct) that installs `CancelToken`-based lifecycle management.

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
#[scheduled(every = "5m", skip_if = "maintenance_mode")] // skip predicate (Quarkus skipExecutionIf)
```

**Skip predicate (`skip_if = "method"`).** The Quarkus `skipExecutionIf` counterpart: names a plain `&self` method (sync **or** async) returning `bool`, defined in the **same** impl block as the `#[scheduled]` method (no route/`#[scheduled]`/`#[consumer]`/`#[async_exec]`/lifecycle marker — enforced with a targeted compile error, as is a non-`&self`-only signature). Evaluated inside the pool job at the start of **every** tick, scheduled and `trigger_now` alike; `true` suppresses the body. The schedule keeps advancing; skips count in `ScheduledJobInfo::skip_count` (`run_count`/`last_run`/`last_duration` only reflect ticks whose body actually ran). For a shared condition (skip-predicate-bean style), `#[inject]` the predicate bean and delegate to it from the method. Dynamic tasks: `ScheduledTaskDef::new(..).with_skip_if(|state| async move { ... })`.

**Overlap policy (`overlap = "skip" | "concurrent"`, default `skip`; also valid with `cron`).** `skip` (today's behavior) re-arms a job on completion, so a tick that comes due while the previous one is still running is skipped — cadence preserved, never overlaps with itself. `concurrent` re-arms at *fire* time (the next deadline is pushed back before the tick is submitted, and completion does not re-arm), so a slow tick never holds back the next; ticks may pile up. Interval cadence stays anchored; cron recomputes next at fire time. Dynamic tasks: `ScheduledTaskDef::new(..).with_overlap(OverlapPolicy::Concurrent)`.

**Config (`scheduler.*`).** Typed `SchedulerConfig` (`CONFIG_PREFIX = Some("scheduler")`, all keys optional): the standard `scheduler.enabled = false` gate skips starting tasks while the provided beans remain; `scheduler.executor = "shared"` (default — the app-wide `PoolExecutor`) or `"dedicated"` (a private pool sized by `scheduler.max-concurrent` / `queue-capacity` / `shutdown-timeout`, mirroring `executor.*`, with its own graceful drain hook). `PoolExecutor` stays a hard `Deps` requirement even in dedicated mode (a type-level requirement cannot be config-conditional). An unrecognized `executor` value panics at boot.

**Runtime control + stats.** `SchedulerHandle` (extract as a handler param, or `SchedulerHandle::channel(token)` to wire it to a manual `start_jobs`) exposes `pause(name).await` / `resume(name).await` / `trigger_now(name).await` (all `-> bool`; `false` means one of: unknown name; shutdown started / no driver — every command refuses once the driver is cancelled, including one already queued, so a command issued from a tick body during shutdown is a no-op and never a deadlock; for `trigger_now`, a `skip` job already in flight, a closed executor pool (answered first, then the driver stops), or a panicking tick factory (contained; the job is disabled); for `resume`, a spent/overflowed schedule. `pause` adds none of its own). A paused job advances its cadence silently but never submits; `trigger_now` fires once out of band (allowed even when paused; its OOB tick never re-arms and leaves the schedule untouched). `ScheduledJobInfo` carries live stats the driver updates: `last_run` / `next_run` (`chrono::DateTime<Utc>`), `last_duration`, `run_count`, `skip_count` (ticks suppressed by a `skip_if` predicate), `panic_count`, `paused` — read via `ScheduledJobRegistry::list_jobs()` / `job(name)`.

**Requires the Executor plugin.** `Scheduler` declares `type Deps = (PoolExecutor,)`, so a chain with `.plugin(Scheduler)` but no `PoolExecutor` bean (normally provided by `.plugin(Executor)`) fails at `build_state()` with the standard guided "missing `.provide::<PoolExecutor>()` or `.register::<PoolExecutor>()`" error. `Deps` are verified against the final provision list, so the order between `.plugin(Executor)` and `.plugin(Scheduler)` does not matter. The `scheduler` facade feature pulls in `executor`.

**Single-driver model.** All schedules are driven by ONE driver task, not one Tokio task per schedule. The plugin builds it as a future (`jobs_driver`) and hands it to `ServeContext::track` from its serve hook, so the driver is a *tracked* task: it owns a clone of the bean graph while it runs (a tick resolving a `GraphHandle` always sees a live graph) and `run()` cancels + joins it on every exit, including a boot aborted by a later startup hook. On cancellation the driver stops arming and waits out its in-flight ticks before completing. `start_jobs` remains as the detached spawn for standalone/test use — not tracked, not graph-owning. The driver owns a min-heap of next-fire deadlines (`ScheduledTask::into_job` → `ScheduledJob`); when the earliest deadline is reached it submits the due tick bodies to the shared `PoolExecutor` and tracks the `JobHandle`s in a `FuturesUnordered`. Under the default `skip` policy a job is re-armed onto the heap only when its own tick completes — so it is either in the heap or in flight, never both; under `concurrent` it is re-armed at fire time and may have several ticks in flight. While a `skip` tick is in flight the job holds no deadline, so `ScheduledJobInfo::next_run` reads `None` until completion republishes one — the field is `Some` **exactly** when a live driver holds a deadline for the job (it is about the deadline, not the fire: a *paused* job publishes the instant it would fire were it resumed, since its cadence keeps advancing and each deadline is re-armed instead of submitted). `next_run` is cleared at the pop that spends a deadline (so it is truthful during tick construction and on a submission that never runs — a closed pool) and for every job when the driver exits, cancelled or pool-closed: once the driver has stopped, nothing can fire, so nothing is published. The driver also accepts runtime `pause`/`resume`/`trigger_now` commands and keeps `ScheduledJobInfo` stats current. `resume` replies `true` iff the job can fire again as far as the driver can tell then: it keeps a live deadline, probes the schedule when an in-flight tick is the only thing due to re-arm the job (a snapshot — a cron's last slot can pass before completion), and otherwise re-arms from now and reports the outcome; a spent/overflowed schedule stays unarmed and replies `false`.

**Pool-tick execution (Quarkus model).** Each scheduled tick runs as a pool job (`executor.submit(...)`). Non-overlap is preserved (`MissedTickBehavior::Skip` semantics — a slow tick blocks only its own schedule), while different jobs still run concurrently (the driver never awaits a tick inline). In-flight ticks drain on shutdown (they are pool jobs covered by `executor.shutdown-timeout` / `PoolExecutor::shutdown_graceful`); the driver breaks on cancellation without aborting them. A panicking tick is contained in the pool job, logged, and its job is re-armed. Scheduled work is globally bounded by `executor.max-concurrent` and appears in `ExecutorMetrics` (running/queued/completed/rejected); when the pool is shut down, the driver stops.

**Registration:** install the `Executor` and `Scheduler` plugins before `build_state()`, then register controllers:
```rust
AppBuilder::new()
    .plugin(Executor)                         // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)                        // install scheduler runtime (provides CancelToken)
    .build_state()
    .await
    .register_controller::<ScheduledJobs>()   // auto-discovers #[scheduled] methods
    .serve("0.0.0.0:3000")
```

The `Controller` trait's `scheduled_tasks_boxed()` method (auto-generated by `#[routes]`) returns type-erased task definitions; `register_controller()` collects them into the shared `TaskRegistryHandle`. Bean scheduled tasks flow through the same registry: `#[bean]` generates a `ScheduledSource` impl and an `after_register` hook (`BeanRegistry::register_scheduled_source`), and `build_state()` drains those hooks against the resolved graph. `serve()` passes all collected tasks to the scheduler backend, which drives the schedules and submits each tick body to the `PoolExecutor`. On shutdown, the `CancelToken` is cancelled.

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

- `cancel()` — cancel the scheduler and all running tasks (triggers the `CancelToken`).
- `is_cancelled()` — check if the scheduler has been cancelled.
- `token()` — get the underlying `CancelToken` clone.

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
- `r2e-data-sqlx` contains cancellation-safe managed SQLx transactions.
  Provide a normal `sqlx::Pool<DB>` and request `Tx<'_, DB>`, or install the
  datasource plugin for a rotating `DbPool<DB>` and request `DbTx<'_, DB>`.
  `DbPool` watches the live URL value as a `ServiceComponent`,
  connects a replacement pool on rotation, swaps atomically on success, and
  closes the old pool in the background. The pool and its generation live in a
  single swapped cell — `snapshot() -> (Pool<DB>, u64)` reads both at once, so
  `DbTx::generation()` can never name a pool the transaction did not run on.
  Since the swap closes the pool it replaces, `DbPool::begin() ->
  Result<(Transaction<'static, DB>, u64), sqlx::Error>` and the
  `Executor for &DbPool` impl retry on `sqlx::Error::PoolClosed` (bounded to
  three attempts, re-reading the snapshot each time), so requests in flight
  during a rotation are not turned into 500s. Transaction sources implement
  `TxSource<DB>` — `begin(&ManagedContext) -> (Transaction<'static, DB>, Meta)`
  — so the rotating source owns the retry while `ManagedTx` keeps the shared
  commit/rollback lifecycle.
- `r2e-data-diesel` contains only managed Diesel/r2d2 transactions and a
  blocking-pool `run` helper. Its `DbPool` has the same atomic `snapshot()`,
  but no retry: rotation only drops the facade's handle and r2d2 pools are
  never explicitly closed, so a handle taken before the swap keeps working.
- **The datasource plugin owns the pool's whole boot.**
  `SqlxDataSource<DB, Tag = DefaultDataSource>` (and its mirror
  `DieselDataSource<Conn, Tag>`) is a `Plugin` with
  `Provided = (DbPool<DB, Tag>,)`, `Deps = (LiveConfigRegistry,)`,
  `CONFIG_PREFIX = Tag::CONFIG_PREFIX`, and `SKIP_BUILD_WHEN_ALL_PINNED = true`.
  `build()` reads the typed `DataSourceConfig` (`url`, `max-connections`,
  `min-connections`, `acquire-timeout`, `migrate-at-start: bool = false`),
  connects the pool from a `LiveConfig<String>` on `<prefix>.url` (the registry
  is a *dep* because the URL must stay live — the typed `url` field exists only
  so a missing key fails with a pointed message), runs the migrator attached by
  `.migrations(&MIGRATOR)` when `migrate-at-start` is true, starts the rotation
  `ServiceComponent` via `ctx.on_serve` + `serve.track`, and closes the pool via
  `ctx.on_shutdown_async` (SQLx only — r2d2 has no async close). Any failure
  aborts the boot as `Plugin 'SqlxDataSource' failed to build: ...`.
- **Tags are how a second database exists.** `DataSourceTag` carries both
  `NAME: Option<&'static str>` and `CONFIG_PREFIX: &'static str` because a
  `const` cannot concatenate `"datasource." + NAME` on stable; the
  `datasource_tag!(pub Reporting = "reporting")` macro mints the pair together.
  The tag is a `PhantomData<fn() -> Tag>` parameter on `DbPool`/`DbTx`/
  `RotatingPool`, defaulted to `DefaultDataSource`, so every pre-plugin
  `DbPool<DB>` spelling still compiles. Each backend owns its own tag trait: an
  app uses one of them, and a datasource marker is not runtime foundation.
- **There is no `datasource.enabled` gate** — a pool bean has no inert form, so
  `enabled = false` only logs a warning. The way to replace a datasource in a
  test is to pin the pool (`override_bean`), which
  `SKIP_BUILD_WHEN_ALL_PINNED` turns into "no connection, no migrations".
- CRUD models and queries remain application-owned and use SQLx or Diesel
  directly.

## Multi-tenancy (r2e-tenant)

User-facing guide: `docs/features/24-tenancy.md`. This section is the internals.

**Module map.** `id` (`TenantId`), `resolver` (SPI #1), `source` (SPI #2 +
`TenantContext`), `map` (`Tenanted<T>`), `router` (`TenantRouter`), `extract`
(`Tenant<T>` / `TenantId` extractors), `plugin` (`Tenancy`, `PerTenant`),
`config` (`TenancyConfig`), `error` (`TenantError` → status).

**Two plugins, both `Plugin`, both factory-first.** Each plugin's
`build` runs inside `build_state()` with the resolver / source bean already
constructed (`Deps` is a real topo edge), so `TenantRouter` and `Tenanted<T>`
are built **directly wired** — no shell/fill. When `tenancy.enabled: false`,
`Tenancy` builds `TenantRouter::disabled(statuses)`; `PerTenant` still builds
a normal `Tenanted<T>` map, but its effects (sweeper, drain hook, eager
preload) are dropped — a disabled router routes nothing into the map anyway. `TenantError::NoSource` (500)
remains for the true wiring bug: injecting `Tenant<T>` for a `T` whose
`PerTenant` plugin was never installed.
`Tenancy` declares `Deps = (R,)`; `PerTenant` declares `Deps = (Src,)`, and
`.fallback_to_default()` switches the impl to `Deps = (Src, T)` so the fallback
bean is compile-checked (the `DefaultFallback` / `NoFallback` marker parameter
selects which `Plugin` impl applies). The fallback is consulted only
after a resolved tenant's `TenantSource::create` returns `Ok(None)`. A missing
tenant is rejected by `TenantRouter`, or becomes `None` for an optional
extractor under the allow policy, before the map is called.

**`TenantRouter` is the "tenancy is installed" witness.** One bean, one
`TypeId`, no generics — required by every per-tenant extractor and every
per-tenant `#[managed]` resource. It owns the resolver, the
`MissingTenantPolicy`, the configured `TenantStatuses`, and the per-request
memo. The `Tenancy` install phase adds a pre-routing layer that parks the
private `TenantMemo(Arc<OnceCell<Option<TenantId>>>)` in `parts.extensions`;
guards, extractors and every managed acquisition share the one in-flight/raw
resolver answer. `None` is memoized before policy application; errors are not.
A bare `TenantId` extension is never authoritative. `TenantRouter::memoized`
is a read-only peek, and `TenantRouter::install_memo(&mut Extensions)` is the
escape hatch for a directly provided/hand-wired router.

**Extractors are `FromRequestPartsVia`, never axum's `FromRequestParts`.** Both
read beans out of the HList state — `TenantRouter` always, plus `Tenanted<T>`
for `Tenant<T>` — and the `HasBean` index witnesses cannot live on the impl
(E0207), so they are parked in the `ViaBean` marker: `ViaBean<I>` for
`TenantId`, `ViaBean<(I, J)>` for `Tenant<T>`. Consequences: a missing plugin
is a compile error at `register_controllers`; and implementing axum's
`FromRequestParts` too would make the marker ambiguous — that invariant is
pinned by `assert_unambiguous_extractor` probes in `tests/tenant/extractor.rs`.
`Option<Tenant<T>>` / `Option<TenantId>` (via `OptionalFromRequestPartsVia`)
mean "no tenant", never "bad tenant". Generated handler heads are snapshotted
after every ordinary `FromRequestParts` parameter, including parameter-level
identity, so `ExtensionTenantResolver` works for managed/guard resolution with
struct- or parameter-level identity. Controller-field tenancy cannot depend on
a parameter-level identity that populates its extension: controller request
data is necessarily extracted first, so it fails closed as missing. Use
struct-level identity plus `#[anonymous]` for public routes in that shape.

**`Tenanted<T>` invariants** (pinned by `tests/tenant/map.rs`):
- *Single flight* — the `Arc<Slot<T>>` is cloned out of the `DashMap` **before**
  any `.await` (a shard guard held across an await would deadlock the map);
  creation runs inside `slot.cell.get_or_try_init` (`rt::sync::OnceCell`).
- *Failures are never cached* — an `Err` removes the empty slot, guarded by
  `Arc::ptr_eq` so a concurrent retry's fresh slot is not stolen. The same drop
  guard removes an empty slot when `create` panics or its caller is cancelled.
  Waiters on an erroring `OnceCell` may retry the initializer one by one because
  errors are deliberately never cached.
- *Unknown tenants are cached briefly* — `Ok(None)` is remembered for
  `negative-ttl`; the negative cache is re-checked inside the initializer, so a
  cold unknown wave makes exactly one source call. Insert-then-bound may exceed
  `max-negative` transiently under concurrency, but each call trims before it
  returns. Any later success clears the entry.
- *Creation is bounded* — `create` runs under `create-timeout`; blowing it is a
  504 and releases every waiter parked on the slot.
- *Idle resources go away* — `Tenanted<T>` is itself a `ServiceComponent`
  (`type Deps = TCons<Self, TNil>`) whose `start` loop sweeps: idle eviction,
  LRU trim to `max-active`, negative purge, each returning a `SweepReport`;
  shutdown `drain()`s. The `PerTenant` plugin starts it.
- *Removal is ready-only* — `evict`, `invalidate`, sweeps and drain leave an
  in-flight creation mapped and return `false` where applicable. Drain latches
  the map closed and repeats ready snapshots with conditional `remove_if`
  (`Arc::ptr_eq`) until none remain. An earlier in-flight creation self-disposes
  its result after observing the latch; later resolutions are 503
  (`the per-tenant resource map is draining (shutdown)`).
- *Disposal is gated per cached value* — the source's `dispose` is called at
  most once even when eviction races drain. `invalidate` means removal plus a
  detached disposal spawn, not completed disposal; outside Tokio it drops
  without calling `dispose` and logs at debug.
- *There are no leases* — `get`, `Tenant<T>` and `into_inner` return clones;
  eviction can dispose while a request still holds one. Resources must tolerate
  close-while-cloned, or disable idle eviction/use `keep_forever`.
- *`max-active` is soft* — completed creations kick a looping detached LRU trim
  and the periodic sweep reinforces it, but creation has no admission bound.
  Cold bursts can exceed the cap, so `db max_connections × max_active` is not a
  hard capacity calculation. Zero is rejected at config load/wiring and by
  `PerTenant::max_active(0)`; `tenancy.enabled: false` is the off switch.

`TenantedMetrics` and `TenantStats` implement `Serialize`; `TenantStats::idle`
is emitted as whole-millisecond `idle_ms`.

**Cascade + cycle detection.** `TenantSource::create` receives a
`TenantContext` carrying the tenant, an `Arc<BeanContext>`, and a
`ResolutionChain` — a `Vec<(TypeId, &'static str)>` cloned per hop (a per-tenant
graph is a handful of types deep, so the copy beats threading a borrow through
boxed futures). `ctx.get::<U>()` pushes onto the chain and refuses a `TypeId`
already in it, reporting `TenantError::Cycle` with a path-stripped chain
(`A -> B -> A`; generic args are kept, so `Pool<Postgres>` and `Pool<Sqlite>`
stay distinct). `ctx.bean::<U>()` is the app-scoped lookup, `ctx.chain()` the
diagnostic string. Detection is per resolution path: two concurrent roots can
form A-awaits-B / B-awaits-A without sharing a chain and wait until
`create-timeout` produces 504 (or hang when disabled). Keep the timeout enabled
in production; a real cycle is a wiring bug sequential resolution exposes.

**`TenantId` has no `Deserialize` impl, deliberately** — it is parsed at the
edge (`[a-z0-9][a-z0-9._-]{0,62}`, `MAX_TENANT_ID_LEN` = 63) so a value that
picks a database/schema/bucket cannot arrive inside a request body and skip
validation. `Serialize` is one-way (returnable, not receivable). `Arc<str>`
inside. `TenantId::from_static(&'static str)` validates the same grammar and
panics on invalid input; there is no unchecked public constructor.

**Backend integration.** `r2e-data-sqlx` / `r2e-data-diesel` gain `tenant.rs`
under their own `tenant` feature: `TenantPools<..> = Tenanted<Pool<..>>`,
`PoolSource` (a `TenantSource` doing tenant → DSN → pool), and a `TenantPool`
`TxSource` marker whose `Deps` list `TenantRouter` + `TenantPools<..>` — which
is why `#[managed] tx: &mut TenantTx<..>` needs no controller field and still
fails to compile without the plugins. The Diesel side has no `dispose` (r2d2
pools have no close) and builds pools in `spawn_blocking`.

**Tests.** `r2e-tenant/tests/tenant/` — `id`, `resolver`, `map`, `cascade`,
`extractor`, `plugin`, with shared `fixtures.rs`. Backend tests:
`r2e-data/backends/{sqlx,diesel}/tests/tx/tenant.rs`. Compile-fail:
`r2e-compile-tests/cases/tenancy/fail/`. Test helpers: `.as_tenant(id)` /
`.as_tenant_user(sub, tenant, roles)` on `TestApp` requests and `TestSession`.
End-to-end example: `examples/example-multi-tenant-db`.

## Cache (r2e-cache)

`TtlCache<K, V>` — thread-safe TTL cache backed by `DashMap`. Supports get, insert, remove, clear, evict_expired.

`CacheStore` trait — pluggable async cache backend. Default: `InMemoryStore` (DashMap-backed). Supports get, set, remove, clear, remove_by_prefix. The store is an application **bean** (`Arc<dyn CacheStore>`): provide one with `.provide(InMemoryStore::shared())`. (The old global `set_cache_backend()`/`cache_backend()` singleton was deleted in Phase 6.)

The `Cache` interceptor (in `r2e-utils`) resolves the store bean at controller registration (`DecoratorSpec` — a missing store is a compile error at `register_controller()`). `#[intercept(Cache::ttl(30).group("users"))]` stores in a named group; `#[intercept(CacheInvalidate::group("users"))]` clears by prefix.

## Rate Limiting (r2e-rate-limit)

`RateLimiter<K>` — generic token-bucket rate limiter keyed by arbitrary type. `RateLimitBackend` trait for pluggable backends (default: `InMemoryRateLimiter`, **single-process**). `RateLimitRegistry` — clonable bean; the `RateLimit`/`PreRateLimit` specs (and their config-resolved twins `ConfiguredRateLimit`/`ConfiguredPreRateLimit`, which additionally depend on `R2eConfig`) pull it once at controller registration into the built guards.

Bucket keys are `<module::path::ControllerName>:<handler>:<kind>` (module-qualified via `module_path!()`) — neither homonymous handlers nor same-named controllers in different modules share a bucket. Kinds: `global` (shared bucket), `user:<sub>` (per authenticated user), `ip:<client-ip>` (leftmost `X-Forwarded-For` entry **that parses as an IpAddr** → `ConnectInfo<SocketAddr>` peer address → `unknown` with a warn-once; a malformed header value counts as absent). `peer_ip_only()` / `trust-forwarded-for: false` ignores the header. Per-user specs set `DecoratorSpec::REQUIRES_IDENTITY = true` (compile error without an identity, 401 at runtime for a `None` optional identity). Zero windows are rejected everywhere (constructor panic; `window-secs: 0` aborts startup), and config keys are read strictly: the default applies only when the key is absent, an invalid value panics at registration. See `docs/book/src/security/rate-limiting.md`.

## OpenAPI (r2e-openapi)

- Generates **OpenAPI 3.1.0** specs. Uses **schemars 1.x** (JSON Schema Draft 2020-12) for schema generation.
- `OpenApiConfig` — configuration for the generated spec (title, version, description). `with_docs_ui(true)` enables the interactive documentation page.
- `OpenApiPlugin` — registers OpenAPI routes. Use `.plugin(OpenApiPlugin::new(config))` on the builder (before `build_state()`; install order is irrelevant — the spec is built from a Routes-stage effect, after every controller is registered).
- `SchemaRegistry` — extra schema collection. `register_for::<T: JsonSchema>()` for schemars types, `register(name, value)` for manual schemas. Wire into `OpenApiConfig` via `with_schema::<T>()`, `with_raw_schema(name, json)`, `with_schema_registry(registry)`, `with_schema_override(name, json)`. Precedence: overrides > route schemas > registry > built-in error schemas.
- `SchemaProvider` — trait for types without `JsonSchema` derive; returns `Cow<'static, str>` name + `Value` schema. Use `SchemaRegistry::register_provider::<T>()` to register.
- Route metadata is collected from `Controller::route_metadata()` via `RouteInfo` (in `r2e-core/src/di/meta.rs`).
- Always serves the spec at `/openapi.json`. When `docs_ui` is enabled, also serves an interactive API documentation UI at `/docs`.
- **Users must add `schemars = "1"` to their Cargo.toml** and derive `JsonSchema` on request/response types. This is required because `schemars_derive` generates code referencing `schemars::` by crate name (same pattern as serde).
- Request body schemas: auto-detected from `Json<T>` params (`application/json`) and `TypedMultipart<T>` params (`multipart/form-data`; schema from the `MultipartSchema` impl generated by `#[derive(FromMultipart)]`, file fields modeled as `type: string, format: binary`). Raw `Multipart` params produce a free-form `multipart/form-data` object body. `Option<Json<T>>` → `required: false`. `RouteInfo.request_body_content_type` carries the media type (`None` = JSON).
- Response schemas: auto-detected from return types (`Json<T>`, `JsonResult<T>`, `Result<Json<T>, _>`). Uses autoref specialization to gracefully skip types missing `JsonSchema`.
- **Unmappable-body warnings (boot-time, once):** when a successful (non-204) response body can't be mapped — an `impl Trait` return or a concrete non-`Json` type — the `#[routes]` macro records the offending return type in `RouteInfo.response_unmapped`, and `build_spec` (which runs once at plugin install / boot) emits a `tracing::warn!` naming method + path + type and pointing at `#[returns(T)]` / `Json<T>`. Named request/response types that lack `JsonSchema` (documented as a generic `object`) are warned about too. Intentional no-body returns (`()`, `StatusCode`, `StatusResult`, `String`, 204) are **not** flagged. Testable seam: `r2e_openapi::spec_warnings(&routes) -> Vec<SpecWarning>` computes the same gaps without logging (`SpecWarning { method, path, gap: SchemaGap }`, plus `.message()`); `SchemaGap` variants: `MissingResponseBody` / `SchemalessResponseBody` / `SchemalessRequestBody`.
- Status codes: smart defaults (GET→200, POST→201, DELETE→204). Override with `#[status(N)]`.
- `#[returns(T)]` — explicit response type for opaque returns (`impl IntoResponse`).
- `#[deprecated]` — standard Rust attribute, reflected in spec.
- Doc comments: first `///` line → `summary`, remaining → `description`.
- 401/403 responses: only emitted when route has auth (`#[roles]`, `#[inject(identity)]`, guards).

## Static File Serving (r2e-static)

`EmbeddedFrontend` — plugin that serves static files embedded in the binary via `rust_embed`, with SPA fallback support. Installs as a fallback handler on the Axum router.

- **Quick start:** `builder.plugin(EmbeddedFrontend::new::<Assets>())` — serves files from a `#[derive(Embed)]` struct with sensible defaults (SPA on, `api/` excluded, `assets/` immutable).
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
- **Install it after the other router plugins** — its SPA fallback is a Graph-stage effect, and Graph effects apply in install order, so a later fallback would shadow it.
- **Feature flag:** `r2e = { features = ["static"] }` or depend on `r2e-static` directly.

## Testing (r2e-test)

- **App boot (the `@QuarkusTest` path)** — apps declare `impl App for MyApp` once in `app.rs`; `lib.rs` includes it for tests and `r2e::app_main!(MyApp)` includes it in the binary tip crate while generating `main`. `setup() -> Result<Env, BootError>` owns long-lived resources and `build(b, env) -> Result<impl BootableApp, BootError>` assembles the app; tests boot the real app **by type** instead of re-declaring controllers:
  - `TestApp::boot::<MyApp>().await` — forces the `test` profile (so `load_config()` overlays `application-test.yaml`) and pins a fresh `TestJwt`'s `Arc<JwtClaimsValidator>`/`Arc<JwtValidator>` over the app's own validator.
  - `TestApp::boot_with::<MyApp>(|b| ...).await` — same, plus a builder hook to pin mocks (`b.override_bean(mock)`) and patch config (`b.override_config_value(key, value)`; or `b.override_config(cfg)` for a full in-memory config). Pinned overrides win over the app's later registrations (first-pin semantics: the harness pre-configures the builder *before* `build` runs, so test overrides must beat later registrations).
  - `TestApp::boot_plain::<MyApp>(|b| ...).await` — skips the TestJwt wiring.
  - **Reusing one `App::Env` (task #988)** — `TestApp::boot_env::<MyApp>(env)` / `boot_with_env::<MyApp>(env, |b| ...)` / `boot_plain_env::<MyApp>(env, |b| ...)` (+ `try_*`) skip `A::setup()` entirely and pass the given `Env` to `A::build`. `App::Env` is `Clone + Send + Sync + 'static`, so a test binary builds it once and boots every test off that one value instead of replaying pools + migrations per test; `#[before_all]` only amortises within one suite. **Memoise it with `r2e_test::SharedEnv<A>`, never a bare `OnceCell`/`LazyLock`** (`shared_env.rs`): `#[r2e::test]` builds one runtime per test and drops it at the end of the test, so a `OnceCell` initialised there binds the environment's reactor (listeners, keep-alive tasks, timers, anything `setup` spawned) to a runtime that dies — the value survives, the reactor does not, and later tests hang. `SharedEnv` is `const`-constructible (`new()` / `with(init)` for setup+seed), runs `setup` exactly once on a process-lifetime multi-thread runtime parked in a `OnceLock` (started from a short-lived `std::thread` calling `RuntimeHandle::block_on`, because `App::setup()`'s RPITIT future has no `Send` bound and cannot be `spawn`ed), publishes the result through a `watch` channel (single-flight for concurrent first callers, cancellation-safe), and remembers a failure instead of retrying it (`SharedEnvError` = app type name + rendered `caused by:` chain); `shared_env_runtime()` exposes the handle. Macro knob: `env = <expr>` on `#[r2e::test(app = ...)]` and `#[r2e::test_suite(app = ...)]` (evaluated in the async block, composes with `with = ...` / `jwt = false`, requires `app = ...`; expands to `boot_with_env` / `boot_plain_env`). All six methods funnel through one private `TestApp::assemble` (harness defaults → hook → `A::build` → `start_in_process`), which never calls `setup` — the plain `boot*` call it themselves. **Isolation is the caller's job**: a shared `Env` is shared state across concurrently-running tests. The harness never disposes the `Env` itself, but it cannot guarantee nothing does — `shutdown()` runs whatever `A::build` registered (disposers, `#[pre_destroy]`, `on_stop`), so an app that hands an `Env`-owned resource to a disposer invalidates it for later boots; documented as the app's contract rather than enforced. Macro diagnostics: `env`/`with`/`jwt` without `app` is an error spanned on the offending argument, and on a suite the same arguments require a `#[before_all]` that binds the booted app (otherwise the expression would never be evaluated). Tests: `r2e-test/tests/boot.rs`, `r2e-test/tests/shared_env.rs` (runtime-death regression, single flight, setup-count, `Env = ()`), `r2e-compile-tests/cases/testing/fail/test_env_without_app.rs` + `test_suite_env_without_before_all.rs` + `test_suite_env_without_app_binding.rs`.
  - **A boot failure is a failing test, not a dead runner.** Both `App` phases are fallible — and so is everything the harness runs around them (config loading, bean/producer construction, plugin build, module/plugin controller registration, and the controller `#[post_construct]`/`#[on_start]` hooks, which `TestApp` runs through `start_in_process`) — so the three `boot*` methods panic with `TestApp::boot::<MyApp>() failed: <error>` plus one `caused by:` line per `source()` level — libtest attributes that to the calling test, and everything already constructed is dropped. This is why `setup`/`build` must never call `std::process::exit`: that code is linked into the test binary and an `exit` there kills the whole run with no attributable failure. `TestApp::try_boot::<A>()` / `try_boot_with` / `try_boot_plain` return `Result<TestApp, BootError>` for tests that assert on a boot expected to fail (`r2e-test/tests/boot.rs`).
  - **The boot runs the production startup phase, and `app.shutdown().await` the production shutdown phase.** `TestApp::boot*` goes through `BootableApp::start_in_process()` (`PreparedApp::start_in_process`, `r2e-core/src/builder/prepared.rs`), the same startup `run()` executes: controller `#[post_construct]` → consumer registrations → bean/controller `#[on_start]` → the builder's `.on_start(…)` closures (so `spawn_service` / `#[derive(BackgroundService)]` tasks are live in tests). It keeps the resulting `RunningApp` (`r2e-core/src/builder/running.rs`), so `app.shutdown().await` runs: `.on_drain(…)` hooks → plugin shutdown hooks + `#[pre_destroy]` disposers → cancel the app token → join the tracked handles under `shutdown_grace_period` (HTTP drain under `drain_timeout`) → `.on_stop(…)` hooks, outside every budget. **`shutdown()` is the OS-signal path**: it does NOT fire `StopHandle::stop()`, because `run_inner`'s shutdown future is `select!(shutdown_signal(), stop_handle.stopped())` — under SIGTERM the handle never fires and `is_stopped()` stays `false` for the whole sequence. The programmatic path is `app.stop_handle().stop()` before `shutdown()`; nothing in R2E flips readiness on its own (only an app's `on_drain` hook does). `shutdown()` is explicit because `Drop` cannot await — dropping a `TestApp` cancels the app token and then **aborts** every live tracked handle (they are never joined afterwards, so dropping them would detach the tasks, not stop them), and warns when there was work pending. `has_shutdown_work()` counts hooks, unfired plugin sync hooks AND live tracked tasks: `false` means dropping loses nothing. It is not literally zero-cost — a start allocates three Arcs (shutdown token, plugin-hook cell, handle collector) — but no hook runs and no task is spawned. **What a test boot skips**: the plugin **serve hooks** (they bind ports — separate-port gRPC, MCP — and start the scheduler driver), hence `#[scheduled]` tasks still do not tick under `TestApp` and WS sessions stay untracked; plus sharded serving and QUIC, which *are* the listener — an in-process start spawns no worker runtimes, so a registered `per_worker_service()` is a boot error there (and an invalid `server.workers` fails the boot on both paths). `app.serve()` binds its `TestServer` on the app's *tracked* lane, so the live server drains with the app, bounded by `drain_timeout` from its own stop as well as from the app's. Tests: `r2e-test/tests/lifecycle.rs`.
  - `#[r2e::test(app = my_app::MyApp)]` — macro form; `app` is the app **TYPE**. Binds test-fn params: `app: TestApp`, `jwt: TestJwt`, `#[inject] bean: T`. Optional `with = |b| ...` and `jwt = false`.
  - **Ordered tests (`@Order`)** — `#[r2e::test(order = <u32>)]` runs tagged tests sequentially in ascending order within the same test **binary** (one file under `tests/`); scope is the binary, never cross-binary/cross-crate. Orders need not be contiguous (10, 20, 30). Works with or without `app = …`; the barrier covers TestApp boot too (no dev-service races). Optional `group = "<name>"` gives independent sequences in one binary — a test waits only on lower orders of its OWN group (default = the unnamed group). Registry filled at binary load via `inventory`; each ordered test waits (synchronous barrier in `r2e-test/src/ordering.rs` — Condvar, real-time clock, immune to `start_paused`) for all lower **registered** orders of its group. Non-ordered tests are untouched and stay parallel (no `--test-threads=1`). **Fail-fast:** a failing ordered test (panic OR `Err` from a `Result` test) poisons its group — later tests fail immediately naming the failed predecessor (no deadlock); a `#[should_panic]` test that panics as expected is a pass and does NOT poison. **Duplicate `order` in a group:** runtime panic naming both tests (macro can't see siblings, so not a compile error). **Watchdog:** a waiting test panics (not hangs) if some lower order was never started and the group stays idle for `R2E_TEST_ORDER_TIMEOUT_SECS` (default 60) — typically a filtered-out lower order or `--test-threads` starvation; a running predecessor never trips it; diagnostic lists pending orders + whether they started. Compile errors: `group` without `order`; `order`/`group` on `#[r2e::main]`. Using `order` requires the `r2e-test` dev-dependency (already present with `app = …`).
  - `app.bean::<T>()` — fetch any bean from the booted app's resolved graph. `app.config()`, `app.test_jwt()` accessors. `.as_user(sub, &roles)` on `TestRequest`/`SessionRequest`/`TestSession` mints a Bearer token from the app's `TestJwt` (the `@TestSecurity` equivalent).
- **Suite-style tests (`#[r2e::test_suite]`)** — J2E-style shared test scope without free functions. Put it on an inherent `impl` block; each `#[case]` method becomes its own Cargo test, while one suite instance is shared behind a per-suite lock. Optional hooks: `#[before_all]` (or `#[beforeAll]`) initializes once and may return `Self` / `Result<Self, E>`; without it the suite uses `Default`. `#[before_each]`, `#[after_each]`, and `#[after_all]` (camelCase aliases accepted) run around cases. The `before_all` constructor can bind `TestApp`, `TestJwt`, and `#[inject]` beans using the same `app = ...`, `with = ...`, and `jwt = false` arguments as `#[r2e::test]`. Cases are unordered by default (Cargo decides start order, but suite access is serialized); `#[case(order = N)]` reuses the ordered-test barrier for ordered cases in that suite. `after_all` (and with it the suite teardown below) fires when the **last generated case completes**, counted against the number of `#[case]`s the macro emitted — libtest does not expose which tests the process selected, so a partial `cargo test <filter>` run legitimately never reaches it and the suite value is leaked to process exit. For the same reason **`#[ignore]` on a `#[case]` is a compile error**: it would either suppress teardown entirely (plain run) or let teardown fire before the ignored case runs (`--include-ignored`). Skip inside the case body instead.

  **One runtime per suite.** The suite's Tokio runtime is built once, owned by the `SuiteCell` alongside the suite value in the generated module's `OnceLock`, and never dropped — `#[before_all]`, `#[before_each]`, every `#[case]`, `#[after_each]` and `#[after_all]` `block_on` that same reactor. This is what makes the suite form worth using: `#[before_all]` exists to amortise expensive setup, and expensive setup is normally runtime-bound (a `TestApp`, a `sqlx` pool, a listening socket, a spawned worker, a timer). A resource registered with a reactor that later disappears does not error — it stops waking — so the pre-fix per-case runtime turned into `PoolTimedOut` several layers away from the cause (Tasker #986). The runtime knobs on `#[r2e::test_suite(flavor = …, worker_threads = …, start_paused = …)]` configure that single runtime; note `start_paused` therefore means one paused clock shared by every case, not a fresh one per case. Guard-rail: every phase — `#[before_all]`, each case, `#[after_each]`, `#[after_all]` — calls `SuiteCell::assert_on_suite_runtime` from inside its `block_on` and panics naming both runtime ids (`rt::RuntimeId`) if it is not on the suite runtime, so a regression in the generated code surfaces as that named panic rather than as a database timeout (it cannot catch a resource bound to a runtime R2E never owned). **Teardown:** the runtime lives in a `Mutex<Option<Runtime>>` inside the cell; a case holds that slot for its whole duration (always locked *before* the state mutex, so the two cannot deadlock), and the case that runs `#[after_all]` then calls `SuiteRuntime::finish` — drop the suite value inside `block_on` (a socket or pool wants its driver present in `Drop`), then `Runtime::shutdown_timeout(1s)`. Otherwise the suite's worker threads and detached tasks would run for the rest of the test process, since the `OnceLock` is never dropped. Anything reaching a torn-down suite gets a named panic from `SuiteRuntime::get`, not a hang. Non-regression tests: `r2e-test/tests/suite.rs`, `r2e-rt/tests/rt/runtime_id.rs`.
- `TestApp` — wraps a `Router` with an HTTP client for integration testing. Methods: `get`, `post`, `put`, `delete`, `patch`, `request` return `TestRequest` builder. Call `.send().await` to execute. `serve()` spawns a live `TestServer` on a random TCP port (needed for WebSocket/SSE) — attached to the app's lifecycle when the `TestApp` was booted. `shutdown()` runs the production shutdown sequence. `from_builder` retains the bean graph (so `bean::<T>()` works); `with_jwt(jwt)` attaches a `TestJwt` to a hand-assembled app.
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
- **Dev services (`r2e-devservices`)** — containerized infra for tests via testcontainers. `DevPostgres` / `DevRedis` / `DevOpenFga` (features `postgres`/`redis`/`openfga`): `shared().await` = one stable container per workspace session, reused across test binaries. A shared `testcontainers/ryuk:0.14.0` instance keeps one TCP lease per process and force-removes labelled services after the final process disconnects (10-second grace by default), then auto-removes itself. `start()` = isolated and handle-scoped, with the same Ryuk session as a crash/`SIGKILL` fallback; `start_with_tag(tag)` selects a custom image tag (defaults pinned to `postgres:16-alpine`/`redis:7-alpine` — the modules' own defaults are pre-arm64). Both take a full spec via `start_with` / `shared_with`: `PostgresImage::new(name, tag)` / `RedisImage::new(name, tag)` for distributions shipping extra extensions (`pgvector/pgvector`, `valkey/valkey`), and `PostgresSpec` (`with_user`/`with_password`/`with_database`, `PostgresImage: Into<PostgresSpec>`) for credentials. Since `SharedIdentity` fingerprints the spec's configuration string, each distinct spec gets its own shared container (one `OnceCell` per configuration, not a single process-wide cell). That string is derived from the `ContainerRequest` itself — image reference + `Image` type, declared/exposed ports, env vars, labels, cmd, entrypoint, mounts, copied files, port mappings, device requests, network, hostname, platform, workdir, user, privileged/caps/shm/… — encoded length-prefixed (keys and values separately) so a value carrying `;` or `=` cannot forge a field boundary. Three rules keep it faithful: **key-resolved** fields (env vars, labels, port mappings) are folded into a `BTreeMap` **in iteration order**, since `env_vars()` yields the image's first and the request's overrides second and Docker keeps the last, and `port_bindings` is a map keyed by container port — folding records the *effective* value and settles the order (some modules hold their env in a `HashMap`, so raw order is not stable across processes); **set-like** fields (exposed ports, mounts, caps) are **sorted**, so declaration order never starts a second container; **ordered** fields (cmd, copy sources, device requests — Moby applies each in turn to the same OCI spec, so a reversal is a different container — and security opts, which Docker parses in sequence so a later `no-new-privileges=false` overrides an earlier `=true`) are left alone. Optional strings are encoded so `None` and `Some("")` stay apart. A copied source is fingerprinted through a streaming digest of its `Debug` form — an in-memory asset never materializes as a decimal-byte string. `testcontainers/device-requests` is enabled unconditionally in `Cargo.toml` (not behind an R2E feature): the identity has to read `device_requests()`, and behind a feature a user enabling it on their own dependency would get GPU reservations invisible to the identity. A host-config modifier is a special case: its *presence* is readable but its effect is a closure, so `shared()` **panics** on a spec that sets one without a discriminator (`ensure_shareable`) instead of merging two containers it cannot tell apart — `start()` is unaffected, nothing is shared there. The request factory must be **deterministic**: it runs again for the identity and on every start attempt, so `start_shared` re-derives each attempt's request (`configuration_of`) and re-runs the modifier guard on it, panicking on a mismatch — otherwise a factory returning a bare request first and a modified one later would start under a name that does not describe it. Other blind spots, all documented on `configuration()` and covered by `with_discriminator`: ulimits (private in testcontainers), the *contents* of a file copied by path (only the path is visible), and anything applied after start (seeded data, plus the `exec_before_ready`/`exec_after_start` hooks an `Image` runs itself). Deliberately excluded: readiness conditions and startup timeout (they change how long we wait, not what runs) and host-port exposures (testcontainers rejects those for reusable containers, and the shared path always asks for reuse). Credentials therefore separate containers on their own (they are `POSTGRES_*` env vars); nothing in the wrappers declares a sharing key. A Postgres image must speak Postgres on 5432 and honour `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`. **Any other service** goes through `DevService`/`DevServiceSpec` (ungated, the generic form all three wrappers are built on): `DevServiceSpec::new("clickhouse", || GenericImage::new(...).with_exposed_port(8123.tcp()).into()).with_port(8123)` then `DevService::shared(spec).await` → `endpoint(8123)`; `testcontainers` and `testcontainers_modules` are re-exported so user specs build against matching versions (module images still need their own feature enabled via the user's own `testcontainers-modules` dependency). `with_port` resolves a port the image exposes, it does not publish one; `with_discriminator` *appends* to the derived sharing key for what the request cannot carry (data seeded after start, the contents of a file copied by path) — it can only split containers, never merge them. testcontainers applies **no** wait strategy when reusing a container, so `DevService` probes each declared port itself — and the probe holds the connection briefly, since Docker's port proxy accepts before the service inside is up. `R2E_DEVSERVICES_KEEP=1` disables the reaper for inspection; Ryuk socket/timeout/session/privileged overrides are documented in `r2e-devservices/README.md`. Wire in via `b.override_config_value("app.database.url", pg.url())` in the boot hook (see `examples/example-postgres/tests/postgres_test.rs` for the `DevPostgres` reference: isolated per-test database on the shared container + migrations applied in the test, so each isolated database is migrated before boot). `DevOpenFga` additionally owns store/model bootstrap (`create_store` / `write_model` / `write_tuples` over OpenFGA's HTTP API) since OpenFGA IDs are server-generated construction-time config; a test creates them and injects `openfga.endpoint` (= `grpc_endpoint()`) / `store_id` / `model_id` before boot (see `examples/example-openfga`). Docker smoke tests: `cargo test -p r2e-devservices --features postgres,redis --test dev_services -- --ignored`.
