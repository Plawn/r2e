# `HttpTrace` — one reusable per-request tracing plugin

Status: **proposal** (2026-09-01). Origin: Tasker #1014 (blumana-survey had to
drop `.plugin(tracing)` entirely to own its request log), #1001 (data-catalog
cannot reproduce its `TraceLayer` shape with the r2e plugins).

## 1. Problem

Since #1010 the entry point (`app_main!` / `launch!` / `#[r2e::main]` /
`#[r2e::test]`) installs the global subscriber from the app's own `tracing:`
section. On the default path the `Tracing` / `ConfiguredTracing` plugins
therefore contribute exactly one thing at runtime: `router.layer(default_trace())`
= `tower_http::TraceLayer::new_for_http()` (`r2e-core/src/runtime/layers.rs:232`).
`Observability` adds the same layer (`r2e-observability/src/lib.rs:172`) **plus**
its own `OtelTraceLayer` span — two spans per request.

tower-http's defaults are wrong for a service that owns its log contract:

| # | Default | Why it hurts |
|---|---|---|
| 1 | `DefaultMakeSpan` records `uri = %request.uri()` | raw URI → path/query secrets (survey `public_token`) land in the logs; unbounded cardinality |
| 2 | no `request_id` on the span | `RequestIdPlugin` exists but the span never sees it → nothing correlates |
| 3 | `DefaultOnFailure` = `ERROR tower_http::trace::on_failure: response failed classification=…` | duplicate line next to the app's own summary |
| 4 | no exclusions | `/health*` + `/metrics` are the most-called routes in any deployment |
| 5 | `on_request`/`on_response` at DEBUG only | invisible in prod (default filter is `info`), noisy in dev — no summary line at INFO |

The only escape hatches today are "don't install the plugin" or
`LOG_FILTER: tower_http::trace=off`. Neither is a design.

## 2. Decision

Split the two responsibilities and give the HTTP half a real plugin:

| Plugin | Owns | After this change |
|---|---|---|
| `Tracing` / `ConfiguredTracing` | the **subscriber** (format, filter, ansi…) | subscriber **only**. No HTTP layer. Still useful for `tracing = false` entry points and non-`launch` apps. |
| **`HttpTrace`** (new, `r2e-core` builtin) | the **per-request span** + summary event + request id + exclusions | the one HTTP tracing layer every app and every other plugin reuses |
| `Observability` | OTLP export + propagation | drops `default_trace()` and `OtelTraceLayer`; installs `HttpTraceLayer` with an OTel `MakeRequestSpan` (one span per request, semconv names, parent from `traceparent`) |
| `RequestIdPlugin` | `x-request-id` in/out + `RequestId` extension | unchanged; `HttpTrace` reuses an existing `RequestId` and mints one otherwise, so install order does not matter and installing both is harmless |

`default_trace()` and the `trace` feature of `tower-http` in `r2e-core` go away.

Why a plugin in `r2e-core` and not a layer users wire by hand: the span is the
correlation backbone (`request_id` + route on every handler log line), so it
has to be the thing `r2e new` scaffolds, `Observability` builds on, and
`TestApp::boot` exercises — i.e. a first-class builtin next to `Cors`,
`Health`, `RequestIdPlugin`.

## 3. Public surface

```rust
use r2e::prelude::*;

AppBuilder::new()
    .plugin(HttpTrace::new())                       // sane defaults, see §4
    .plugin(HttpTrace::builder()
        .exclude_path("/internal")
        .capture_header("user-agent")
        .record_path(true)                          // opt IN to the raw path
        .build())
```

Typed config, `CONFIG_PREFIX = "trace"`, enabled gate `trace.enabled`:

```yaml
trace:
  enabled: true
  exclude-paths: ["/health", "/metrics"]   # prefix match, raw path OR route label
  request-id: true                          # read x-request-id or mint a UUID, echo it back
  record-path: false                        # raw path on the summary event (never on the span)
  record-query: false
  capture-headers: []                       # recorded as `header.<name>` on the span
  summary: true                             # one INFO line per request
  request-event: false                      # opt-in DEBUG "request started" line
```

Precedence per knob: **explicit builder > app file > preset > built-in
default**. The first two and the last are the existing `Prometheus` rule; the
`preset` slot is new — see §12: it is what lets a shared company crate ship
its setup while every app's `application.yaml` still wins. `enabled: false` →
inert: no layer at all (surface effect dropped by the gate), no request-id
minting.

Naming: `trace` was chosen over `access-log` / `http-trace` because hyphenated
top-level keys are not addressable by the `R2E_` env overlay (strict `_`→`.`),
and `trace.enabled` is exactly the knob an ops team flips per environment.
`tracing:` = where logs go; `trace:` = what each request logs. Alternatives
considered: `requests`, `access` (ambiguous with authz), `http` (`http.enabled`
reads as "turn HTTP off").

## 4. What it emits

Span, name `request`, level INFO, target `r2e::http`:

| field | value | note |
|---|---|---|
| `method` | `GET` | bounded (`method_label`) |
| `route` | `/users/{id}` or the unmatched sentinel | `MatchedPath`, never the raw path — `add_layer` layers run after routing so it is always there (`builder/typed.rs:995`) |
| `request_id` | uuid / inbound `x-request-id` | absent when `request-id: false` |
| `header.<name>` | inbound header value | only for `capture-headers` |

Summary event, once per request, target `r2e::http`, emitted **inside** the span:

```
INFO r2e::http: request completed status=200 latency_ms=3.2
INFO r2e::http: request completed status=404 latency_ms=0.4
ERROR r2e::http: request completed status=503 latency_ms=12.0
```

- level: INFO below 500, ERROR at 5xx. No separate "failure" event (fixes #3).
- `path=` / `query=` appended only when `record-path` / `record-query` are on —
  they live on the **event**, not the span, so a secret never propagates to
  every handler log line even when opted in.
- `latency_ms` is measured to the response **head** (same as tower-http);
  streaming bodies are not included — documented, and the reason the
  Prometheus layer keeps its own timer.
- Excluded paths get **no span and no event** (not "a span with a filter"), so
  handler logs under `/health` are not even decorated.

With the pretty subscriber a handler line therefore reads
`INFO request{method=GET route=/users/{id} request_id=…}: my_app: loaded user`.

## 5. Extension point — `MakeRequestSpan`

The layer builds its span through a trait so `Observability` (and an app with
its own span shape) reuse everything else — exclusions, request id, status
recording, summary — and replace only the span:

```rust
pub trait MakeRequestSpan: Send + Sync + 'static {
    /// Build the span for one request. `route` is the bounded label and
    /// `request_id` is already resolved.
    fn make_span(&self, req: &RequestHead, route: &str, request_id: Option<&str>) -> tracing::Span;

    /// Record the outcome on the span AND emit the summary event.
    /// Default impl = the §4 shape (`status` field, `request completed`
    /// event, INFO/ERROR split). One method owns both the field names and
    /// the event shape — there is deliberately no "the layer records
    /// `status` by name, declare it Empty" contract: a custom span with
    /// different field names overrides this too, and nothing is lost
    /// silently.
    fn on_response(&self, span: &tracing::Span, outcome: &RequestOutcome);
}

/// What the layer measured: `status: Option<StatusCode>` (None on transport
/// error — unreachable under axum's `Infallible` services, kept for the
/// generic tower contract), `latency: Duration`.
pub struct RequestOutcome { /* status, latency */ }
```

- `DefaultRequestSpan` — §4 shape, short field names, good for `fmt`.
- `r2e_observability::OtelRequestSpan` — semconv names (`http.request.method`,
  `http.route`, `otel.kind = "server"`), parent context extracted from the
  inbound headers (`set_parent`), `capture-headers` as `header.*`; its
  `on_response` records `http.response.status_code` and keeps the default
  summary event. Replaces `OtelTraceLayer` + `middleware.rs` entirely.
- `HttpTrace::builder().make_span(MySpan)` for apps.

The trait takes `RequestHead` (`r2e-core/src/web/request_head.rs`) — the
R2E-owned view — not `http::Request`, so it stays inside the http containment
boundary.

## 6. Layer internals

New module `r2e-core/src/runtime/http_trace.rs` — a tower `Layer`/`Service`
pair with a `pin_project_lite` response future, modelled on
`r2e-prometheus/src/layer.rs` and the current `OtelTraceLayer`:

1. `call`: prefix-match exclusions on raw path **or** route label (identical
   semantics to `prometheus.exclude_paths`, shared helper in `r2e-http`
   `labels.rs`) → early return `inner.call(req)` untouched.
2. Request id: reuse `req.extensions().get::<RequestId>()` if present, else
   `fresh_request_id()` (moved to `pub(crate)`), insert the extension, remember
   the `HeaderValue` for the response.
3. `make_span(...)`, then `inner.call(req).instrument(span)` — the span is
   **entered for the whole handler future**, which is what puts
   `request_id`/`route` on every handler log line (the current `OtelTraceLayer`
   only enters it in `poll`, which is equivalent but hand-rolled).
4. On `Ready(resp)`: `make_span.on_response(&span, &outcome)` (records
   status + emits the summary — see §5), then echo `x-request-id`.

Config structs: `HttpTraceConfig` (`#[derive(ConfigProperties)]`, kebab keys)
in `r2e-core/src/builtins/http_trace.rs` next to `HttpTrace` / `HttpTraceBuilder`.
`exclude_paths: Arc<[String]>`, `capture_headers: Arc<[HeaderName]>` — cloned
by refcount per request, headers pre-validated at build (an invalid header name
is a boot error, not a per-request `if let`).

## 7. Interactions

- **Ordering**: `add_layer` layers wrap in install order (later = outer). The
  layer reads `MatchedPath` and `RequestId` from extensions and mints the id
  itself when missing, so no ordering constraint is documented for users.
- **Prometheus**: independent layer, own timer; both use the same
  `exclude_paths` prefix semantics so one mental model covers both.
- **gRPC multiplexed / raw QUIC**: bypass HTTP layers as today; not this
  plugin's concern (no change).
- **MCP**: routes are plain HTTP → traced like any route; `route` is the MCP
  mount path.
- **`TestApp::boot`**: runs `App::build`, so the layer is installed in tests
  exactly as in prod. `r2e-test` keeps installing its own quiet subscriber.
- **`r2e new`** template: `.plugin(Tracing)` → `.plugin(HttpTrace::new())`
  (the subscriber now comes from `app_main!`).
- **dev-reload**: the layer is rebuilt per patch like every `add_layer`; no
  process-global state (unlike the subscriber), nothing to pin.

## 8. Breaking changes (R2E is pre-production; listed so they are acknowledged)

1. `Tracing` / `ConfiguredTracing` no longer install any HTTP layer. Apps
   relying on the tower-http `request` span must add `.plugin(HttpTrace::new())`.
2. `r2e_core::runtime::layers::default_trace()` removed; `tower-http` feature
   `trace` dropped from `r2e-core`.
3. `Observability` emits one span per request (the `HttpTrace` one) instead of
   the tower-http `request` span + the `HTTP request` OTel span.
   `r2e_observability::middleware::OtelTraceLayer` removed.
4. The tower-http `tower_http::trace::*` targets disappear from the logs;
   `LOG_FILTER` directives naming them become no-ops. New target: `r2e::http`.

## 9. Files

| Area | Change |
|---|---|
| `r2e-core/src/runtime/http_trace.rs` | new: `HttpTraceLayer`, `HttpTraceService`, response future, `MakeRequestSpan`, `DefaultRequestSpan` |
| `r2e-core/src/builtins/http_trace.rs` | new: `HttpTrace`, `HttpTraceBuilder`, `HttpTraceConfig` (`CONFIG_PREFIX = "trace"`) |
| `r2e-core/src/builtins/mod.rs` | `Tracing`/`ConfiguredTracing::build` → no layer; doc table rewritten; export |
| `r2e-core/src/builtins/request_id.rs` | `fresh_request_id` shared; doc: "`HttpTrace` includes this" |
| `r2e-core/src/runtime/layers.rs` | remove `default_trace`; fix stale `info,tower_http=debug` doc-comment (`:61`) |
| `r2e-core/src/prelude.rs`, `lib.rs` | export `HttpTrace`, `HttpTraceBuilder`, `MakeRequestSpan` |
| `r2e-http/src/labels.rs` | shared `path_excluded(raw, label, &[String]) -> bool` (used by prometheus + http_trace) |
| `r2e-observability/src/{lib,middleware}.rs` | `OtelRequestSpan: MakeRequestSpan`; `build` installs `HttpTraceLayer::new(cfg).make_span(OtelRequestSpan)`; `capture_headers` moves onto the shared config |
| `r2e-cli/src/commands/templates/project.rs` | scaffold `.plugin(HttpTrace::new())` |
| `docs/book/src/advanced/observability.md`, `docs/claude/subsystems.md`, `llm/observability.md` (+ `builder-method-quick-reference.md`, `quick-start.md`), `llm-full.txt` via `scripts/check-llm-docs.sh --update` | docs |

## 10. Tests (`r2e-core/tests/http/http_trace.rs`, one target module)

- span carries `route` template + `request_id`, never the raw path (route with
  a secret-looking segment; assert the captured log line does not contain it).
- inbound `x-request-id` reused and echoed; minted otherwise; same id when
  `RequestIdPlugin` is installed before **and** after `HttpTrace`.
- exclusions: `/health` produces no span and no event; prefix matches both raw
  path and route label.
- summary level: 200 → INFO, 503 → ERROR, exactly one event per request.
- `record-path: true` puts `path=` on the event only.
- `trace.enabled: false` → no layer, no `x-request-id` header.
- builder > file precedence for `exclude-paths`.
- `r2e-observability`: one span per request, `http.route` set, parent from
  `traceparent` (existing propagation tests re-pointed at the new layer).
- `r2e-cli/tests/new_project.rs`: scaffold compiles with `HttpTrace`.

Log capture: `tracing_subscriber::fmt().with_writer(MakeWriter)` into a shared
buffer under `support::env_lock()` (the subscriber is process-global — one
`try_init` per test binary, assert on the buffer).

## 11. Open question

`exclude-paths` default `["/health", "/metrics"]` vs empty. Proposal: the
non-empty default, because both paths are R2E's own plugins' defaults and probe
+ scrape noise is the #1 reason people turn request logging off; an app that
mounts something else under `/health` sets `exclude-paths: []`. `Prometheus`
keeps its empty default (excluding a path from metrics silently is worse than
excluding it from logs).

## 12. Presets — one setup imported by every project

Goal (2026-09-02): configure once, reuse the identical HTTP-log contract in
every company service, while each service's `application.yaml` can still
deviate. This is the Spring Boot starter / Quarkus extension-defaults story.

Two pieces, one of which already exists:

**a) The preset lane on the plugin.**

```rust
impl HttpTrace {
    /// A full `HttpTraceConfig` that fills the DEFAULT slot of the
    /// precedence chain: explicit builder knob > app file > preset >
    /// built-in default. A baseline crate ships its contract here; the app
    /// keeps the last word in YAML without touching code.
    pub fn preset(cfg: HttpTraceConfig) -> Self;
}
```

Mechanically one extra argument to `resolve_config` (the `Prometheus`
`builder-vs-file` merge, `r2e-prometheus/src/lib.rs:304`, grows a third
source). Why not plain builder methods in the baseline crate: builder beats
file, so the baseline would silently override every app's YAML — backwards.
Spring Boot has the same ordering (library-contributed defaults <
`application.yml` < explicit code), Quarkus gives extension config a lower
ordinal than the app's `application.properties`.

Convention to generalize later (not in this change): any plugin whose config
merges builder+file should expose the same `preset(...)` slot.

**b) The delivery vehicle — a module that brings the plugin (exists today).**

```rust
// company crate `blumana-baseline`
pub fn http_trace() -> HttpTrace {
    HttpTrace::preset(HttpTraceConfig {
        exclude_paths: Some(vec!["/health".into(), "/metrics".into(), "/docs".into()]),
        capture_headers: Some(vec!["x-tenant".into()]),
        ..Default::default()
    })
}

#[module(plugins(HttpTrace = blumana_baseline::http_trace(),
                 Prometheus = blumana_baseline::prometheus()))]
pub struct Baseline;
```

Every service: `.register_module::<Baseline>(..)` — the brought plugins join
the app-global provision list as if `.plugin(..)` sat at the call site, and
the one-owner rule turns a double install into a named boot error
(`docs/claude/plugins.md` § Modules and plugins). Nothing new to build here;
the recipe gets documented in the book + `llm/plugins.md` once the preset
lane exists.

## 13. J2E precedent — what is adopted, what is deliberately not

| J2E concept | Their shape | Here |
|---|---|---|
| Quarkus `quarkus.http.access-log.*` | enable flag, exclusions, pattern | `trace.*` section — same knob set, same "one section = the whole request-log contract" |
| MDC + `%X{requestId}` in the pattern | thread-local map + pattern refs | span fields (`request_id`, `route`) — structurally the same correlation, but typed and on every handler line for free |
| Sleuth/Micrometer traceId in logs | agent/starter | `OtelRequestSpan` (§5) — trace id lands on the span when `Observability` is installed |
| Spring Boot starter with config defaults | auto-configuration, defaults below `application.yml` | `preset(...)` lane + baseline module (§12), same precedence |
| Access-log **pattern string** (`%h %l %u %t "%r" %s %b`, named `common`/`combined`) | printf-style line | **rejected**: a text pattern is the pre-structured-logging answer — it fights `format: json`, loses typed fields, and reintroduces the raw-URI leak (`%r`) the whole design exists to remove. The configurable field set (`record-path`, `record-query`, `capture-headers`, `summary`) is the structured equivalent. If a CLF-formatted file is ever a hard requirement (legacy log shipper), that is a custom `MakeRequestSpan`, not a core knob. |
| `exclude-pattern` (regex) | regex on the path | prefix list, shared semantics with `prometheus.exclude_paths` — bounded and grep-able; regex can be added later behind the same key without breaking |
