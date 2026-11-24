# Feature 26 — HTTP Stack: Layer Bands, CORS and Request Tracing

## TL;DR

Every HTTP layer lives in a **band** (`LayerSlot`): `Trace > Metrics > Cors > Auth > App`, outermost first. The band decides where a layer sits, not the `.plugin(..)` order. `.plugin(Cors::new())` reads the typed `cors.*` section: origins must be a list, entries are validated at boot, and CORS is **fail-closed**, so missing origins abort startup everywhere except under the `dev` profile. `.plugin(HttpTrace::new())` gives one span and one `request completed` line per request, configured by `trace.*`. Code deep in a service enriches the current request's span with `with_request` / `with_request_state`, with no HTTP type in its signature. The same precedence rule applies to every configurable plugin: **builder setter > file section > preset > default**.

## Overview

The HTTP stack of an R2E app is built from a handful of plugins (`HttpTrace`, `Prometheus`, `Cors`, `SecureHeaders`, …) and from the app's own layers (`with_layer`, `with_layer_fn`, a plugin's `ctx.add_layer`). This guide covers:

1. **Layer bands** — where each layer ends up, and how to put a layer of your own in a specific band.
2. **CORS** — the `Cors` plugin and its typed `cors.*` section.
3. **Security headers** — `SecureHeaders`.
4. **Request tracing** — the `HttpTrace` plugin, its `trace.*` section, and request-scoped span enrichment (`with_request`, `with_request_state`).

Metrics (`Prometheus`, `MetricsFacade`) and OpenTelemetry export (`Observability`) live in their own crates and install into the same bands. See the book chapters *Observability* and *Prometheus* (`docs/book/src/`).

## 1. Layer bands (`LayerSlot`)

`LayerSlot` is in the prelude. From the outside in:

| Band | Installed by | Why there |
|---|---|---|
| `LayerSlot::Trace` | `HttpTrace`, `RequestIdPlugin`, `Observability` | sees every request, including rejected ones |
| `LayerSlot::Metrics` | `Prometheus`, `MetricsFacade` | counts every request, including CORS preflights and auth rejections |
| `LayerSlot::Cors` | `Cors`, `SecureHeaders` | answers preflights, and adds headers to gate rejections too |
| `LayerSlot::Auth` | your own gates (`ctx.add_layer_in(LayerSlot::Auth, ..)`) | rejects early, inside CORS |
| `LayerSlot::App` (default) | `AppBuilder::with_layer`, `with_layer_fn`, `PluginBuildContext::add_layer` | the app's own middleware, closest to the routes |

Install order matters only **within** a band, where a later layer is outer. Three methods take an explicit band:

```rust,ignore
use r2e::prelude::*;
use r2e::http::layers::cors::{Any, CorsLayer};

AppBuilder::new()
    // App band (innermost) — the default
    .with_layer(my_timeout_layer)
    // a hand-built CORS layer belongs in the Cors band, outside any auth gate
    .with_layer_in(LayerSlot::Cors, CorsLayer::new().allow_origin(Any))
    // a router transformation in a chosen band
    .with_layer_fn_in(LayerSlot::Trace, |routes| routes.layer(my_access_log))
```

Inside a plugin's `build`, `ctx.add_layer_in(LayerSlot::Auth, |routes| ..)` does the same.

Routes merged through `with_layer_fn` (`merge` / `nest`) are wrapped by the four outer bands, but **not** by `App` layers installed before them.

These are outside the band model and unchanged by it:
- per-route `#[layer(..)]`
- guards and interceptors
- trailing-slash normalization
- the two catch-panic layers (one right above the routes, one outermost)
- `DeferredContext::wrap_router` (transport wraps, outermost)

The diagram is in [13-lifecycle-injection-performance.md § 1.3](./13-lifecycle-injection-performance.md#13-tower-layer-order). Upgrading from an older stack that relied on plugin order: [`docs/migration/layer-slots.md`](../migration/layer-slots.md).

## 2. CORS (`Cors` plugin)

```rust,ignore
use r2e::prelude::*;

AppBuilder::new()
    .load_config::<AppConfig>()
    .plugin(Cors::new())               // reads `cors.*`
    .build_state()
    .await
```

The section is loaded inside `build_state()`, so `.plugin(Cors::new())` can go anywhere in the chain, before or after `load_config()`.

### The `cors.*` section

Every key is optional (typed view: `r2e::builtins::cors::CorsConfig`):

```yaml
cors:
  allowed-origins:            # a YAML list: "*" | exact origin | scheme://host:*
    - https://app.example.com
    - http://localhost:*
  allowed-methods: [GET, POST, PUT, DELETE]       # unset = any method
  allowed-headers: [authorization, content-type]  # unset = any header
  expose-headers: [x-request-id]
  allow-credentials: true
  max-age: 2h                 # preflight cache: 7200 or "2h"
```

| Key | Type | Notes |
|---|---|---|
| `allowed-origins` | list | see grammar below; **required** outside `dev` |
| `allowed-methods` | list | method names; an invalid one is a boot error |
| `allowed-headers` | list | header names; an invalid one is a boot error |
| `expose-headers` | list | response headers the browser may read |
| `allow-credentials` | bool | default `false`; see the credential rules below |
| `max-age` | duration | `Access-Control-Max-Age` |
| `enabled` | bool | `cors.enabled: false` installs no layer at all |

**Origin grammar.** Each entry is one of these forms:
- `"*"`
- `scheme://host`
- `scheme://host:port`
- `scheme://host:*`, which matches any port

Bracketed IPv6 hosts are accepted. Entries are validated at boot: a path (`http://app.example.com/`), a query, a missing scheme, an empty host or a bad port fails `build_state()` and names the entry. The same grammar is used by `mcp.allowed-origins`. A `${VAR}` placeholder expands to **one** entry. It is not split on commas, so list each origin separately.

**Credentials.** `allow-credentials: true` cannot be combined with any wildcard. Browsers reject wildcards on credentialed requests, so each of these is refused at boot instead of failing at request time:
- `"*"` among the origins
- an unset `allowed-methods` (which means any method)
- an unset `allowed-headers` (which means any header)

### Fail-closed origins

When no tier sets `allowed-origins`, the plugin refuses to boot with an error naming the key. The only exception is the `dev` profile: there it falls back to permissive (any origin) and logs a warning. An **empty** list is always an error.

### Constructors and setters

| Constructor | Behaviour |
|---|---|
| `Cors::new()` / `Cors::default()` | configured by `cors.*`; setters override the file |
| `Cors::preset(CorsConfig { .. })` | a shared baseline **below** the file: the app's section overrides it key by key |
| `Cors::permissive()` | `Cors::new().allowed_origins(["*"])`. The origin is set explicitly, so it wins over the file. Meant for development |
| `Cors::custom(layer)` | installs a hand-built `r2e::http::layers::cors::CorsLayer` as-is; ignores every `cors.*` key except `cors.enabled` |

Setters, all on the explicit (top) tier unless noted:
- `allowed_origins(..)`
- `allowed_methods(..)`
- `allowed_headers(..)`
- `expose_headers(..)`
- `allow_credentials(bool)`
- `max_age(Duration)`
- `default_origin(s)` / `default_origins(..)`: preset tier. This is the fallback when neither the builder nor the file sets origins, for example the local frontend an app ships with:

```rust,ignore
.plugin(Cors::new().default_origin("http://localhost:5173"))
```

`Cors` installs into `LayerSlot::Cors`. Do not also add a hand-built CORS layer with `with_layer`: it would land in the `App` band and answer preflights a second time. Use `Cors::custom(..)` or `with_layer_in(LayerSlot::Cors, ..)` instead.

## 3. Security headers (`SecureHeaders`)

```rust,ignore
.plugin(SecureHeaders::default())
.plugin(SecureHeaders::builder()
    .hsts(true)
    .hsts_max_age(63_072_000)
    .frame_options("SAMEORIGIN")
    .content_security_policy("default-src 'self'")
    .build())
```

The defaults set these headers:
- `X-Content-Type-Options: nosniff`
- `X-Frame-Options: DENY`
- `Strict-Transport-Security: max-age=31536000; includeSubDomains`
- `X-XSS-Protection: 0`
- `Referrer-Policy: strict-origin-when-cross-origin`

The builder also has `no_frame_options()`, `hsts_include_subdomains`, `xss_protection`, `referrer_policy`, `permissions_policy` and `content_type_options`. It installs into `LayerSlot::Cors`.

## 4. Request tracing (`HttpTrace`)

```rust,ignore
AppBuilder::new()
    .plugin(HttpTrace::new())                    // defaults + `trace.*`
    .plugin(HttpTrace::builder()                 // or: explicit settings
        .exclude_path("/internal")
        .capture_header("user-agent")
        .record_path(true)
        .build())
```

Per request it emits a span named `request` (INFO, target `r2e::http`), entered for the whole handler future. The span carries:
- `method`
- `route`: the bounded route template, never the raw path
- `request_id`
- the captured headers

Inside the span it emits one summary event:

```text
INFO  r2e::http: request completed status=200 latency_ms=3.2
ERROR r2e::http: request completed status=503 latency_ms=12.0
```

`latency_ms` is measured to the response head (streaming bodies excluded). The **subscriber** (format, filter, ANSI) is a separate concern: the `tracing:` section, installed by `build_state()`. In short, `tracing:` says *where logs go* and `trace:` says *what each request logs*.

### The `trace.*` section

```yaml
trace:
  enabled: true                            # false = no layer: no span, no event, no request id
  exclude-paths: ["/health", "/metrics"]   # default; prefix match on the raw path OR the route label
  request-id: true                         # read x-request-id or mint a UUID v4, echo it back
  record-path: false                       # raw path on the summary event (never on the span)
  record-query: false
  capture-headers: []                      # recorded on the span; an invalid name is a boot error
  summary: true                            # INFO below 500, ERROR at 5xx
  request-event: false                     # opt-in DEBUG "request started" line
```

Excluded paths get no span and no summary, but the request id is still resolved and echoed. To trace something mounted under `/health` or `/metrics`, set `exclude-paths: []`.

Builder equivalents:
- `exclude_path(s)`
- `request_id(bool)`
- `record_path` / `record_query`
- `capture_header(s)`
- `summary`
- `request_event`
- `preset(cfg)`
- `make_span(M)`: replaces the span shape with an implementation of `MakeRequestSpan`

`HttpTrace::preset(HttpTraceConfig { .. })` sets a baseline below the file. `HttpTrace` installs into `LayerSlot::Trace`. `Observability` (OpenTelemetry) installs the same layer with an OTel span shape, so there is still exactly one span per request.

### Enriching the request span from anywhere: `with_request`

A handler can take `RequestSpan` as a parameter and record on it. Code further down (a service, a repository, a helper shared with background jobs) uses the task-local request context instead, with no HTTP type threaded through:

```rust,ignore
use r2e::prelude::*; // with_request, with_request_state, SpanState

fn record_order(order_id: Uuid) {
    // Returns None (and does nothing) outside a traced request.
    with_request(|span, _state| {
        span.record("order_id", tracing::field::display(order_id));
    });
}
```

Rules:

- **Record on the span handed to the closure**, not on `Span::current()`. The caller is usually inside nested spans (`#[instrument]`, an interceptor) that never declared the field.
- The field must be **declared** (as `tracing::field::Empty`) by the span shape, i.e. your `MakeRequestSpan::make_span`. A field the span did not declare is silently ignored (`tracing`'s own contract). The default span shape declares no app fields.
- `with_request` returns `None` without calling the closure in these cases:
  - outside a request (a scheduled job, an event consumer, a unit test)
  - on an excluded path, or with `trace.enabled: false`
  - in a task **spawned** from the handler (`rt::spawn`, `#[async_exec]`)

  The context lives exactly as long as the handler future, so clone what a spawned task needs first (`span.clone()`, `SpanState::get_arc`).

### Per-request state: `with_request_state`

Span fields are write-only, so a value produced during the request cannot be read back for a custom summary line. `MakeRequestSpan::make_state` allocates a per-request `SpanState` slot (`SpanState::new(value)`). The layer hands that slot back to `on_response`. `with_request_state::<T, _>` reaches it from any depth:

```rust,ignore
#[derive(Default)]
struct Facts { order_id: std::sync::Mutex<Option<Uuid>> }

with_request_state(|span, facts: &Facts| {
    span.record("order_id", tracing::field::display(id));   // every later line
    *facts.order_id.lock().unwrap() = Some(id);             // read back in on_response
});
```

It returns `None` in every case where `with_request` does, and also when the slot is empty or holds another type. It never panics. `SpanState::get::<T>()` borrows the value and `get_arc::<T>()` clones an `Arc<T>` for a spawned task.

A `MakeRequestSpan` implementation has three methods:
- `make_span(&self, req: &RequestHead, route, request_id) -> tracing::Span`
- `make_state(&self, req) -> Option<SpanState>`: optional, `None` by default
- `on_response(&self, span, outcome: &RequestOutcome, state)`: optional; the default records `status` and emits the summary line

Install it with `HttpTrace::builder().make_span(MySpan).build()`.

## 5. Precedence: builder > file > preset (`Layered<C>`)

`Cors`, `HttpTrace`, `Prometheus`, `MetricsFacade` and `McpServer` resolve their settings the same way, per key:

```text
builder setter  >  file section (cors.*, trace.*, …)  >  preset (Plugin::preset(cfg))  >  built-in default
```

A preset is a shared baseline, for example a company-wide policy crate. The app's YAML overrides it key by key, and code overrides both. The mechanism (`r2e::config::Layered<C>` and `ConfigProperties::overlay`) is documented in [01-configuration.md § Layered plugin configuration](./01-configuration.md#layered-plugin-configuration).
