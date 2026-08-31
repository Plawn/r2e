# Metrics stacks: `prometheus` registry vs the `metrics` facade

Design note for W17 / F13 ("Prometheus stack mismatch"). It records what the
`r2e-prometheus` plugin owns, why an application already on the
[`metrics`](https://docs.rs/metrics) facade could not use it, and the
decomposition that was shipped.

## 1. The two stacks

| | `prometheus` crate (what R2E shipped) | `metrics` facade (what the app had) |
|---|---|---|
| model | you hold typed collectors (`IntCounter`, `HistogramVec`) and register them into a `Registry` | you call `counter!("name", "k" => v).increment(1)`; a process-global **recorder** decides what happens |
| exposition | `TextEncoder` over the registry | an exporter crate (`metrics-exporter-prometheus`) renders a handle |
| who owns the endpoint | whoever holds the registry | whoever holds the exporter handle |
| coupling | metric emission is coupled to the registry that owns it | emission is decoupled from the backend (that is the whole point of a facade) |

The observed mismatch (blumana/data-catalog): the app is on
`metrics` + `metrics-exporter-prometheus`, so it could not use
`r2e_prometheus::Prometheus` at all — the plugin would install a *second*
metrics stack, with a second `/metrics` endpoint, in the same process. The app
therefore hand-wrote an `http_metrics` tower middleware plus its own `/metrics`
route, duplicating what R2E already knows how to do (matched-path labels,
exclusions, in-flight accounting, cancellation-safe timing).

## 2. What the plugin actually does — three responsibilities

Before this sprint, `Prometheus` did three separable things, all-or-nothing:

1. **HTTP tracking layer** — a tower `Layer` that times requests, labels them
   with the *matched route template* (`/users/{id}`, never the concrete URL) and
   the bounded method/status label sets from `r2e_core::http::labels`, honours
   `exclude_paths`, and keeps an in-flight gauge balanced through an RAII guard
   (so a cancelled request still decrements).
2. **Registry / recorder ownership** — the process-global `prometheus::Registry`
   singleton, its default collectors, the histogram buckets, the namespace, plus
   the `PrometheusRegistry` bean apps use to register their own collectors.
3. **`/metrics` endpoint** — a route rendering the registry with `TextEncoder`.

(1) is framework knowledge worth reusing on any stack. (2) and (3) are stack
choices that belong to the application when it already made them.

## 3. Chosen decomposition

Three modes, one crate:

| mode | layer | registry/recorder | `/metrics` |
|---|---|---|---|
| `Prometheus::new("/metrics")` (default, unchanged) | R2E | R2E (`prometheus`) | R2E |
| `Prometheus::layer_only()` — a.k.a. `prometheus.expose_endpoint: false` | R2E | R2E (`prometheus`) | app |
| `MetricsFacade` (feature `metrics-facade`) | R2E | **app** (`metrics`) | app |

### 3a. Endpoint ownership is a knob, not a type

`expose_endpoint` exists both as a builder setting
(`Prometheus::builder().without_endpoint()`, or the shorthand
`Prometheus::layer_only()`) and as a config key (`prometheus.expose_endpoint`),
resolved by `resolve_expose_endpoint` with the crate-wide precedence rule
**builder setting > file config > default**, default `true`. Both spellings
because every other knob of this plugin already has both (`endpoint`,
`namespace`, `buckets`, `exclude_paths`), and endpoint ownership is exactly the
kind of thing an ops profile wants to flip without a rebuild.

This mode still owns the registry: the app keeps `PrometheusRegistry`, registers
its own collectors, and scrapes via `encode_metrics()` from its own route, a
push gateway, or a sidecar. It composes with the W15 `enabled` gate the usual
way — `prometheus.enabled = false` drops *all* surface effects (layer included),
`expose_endpoint = false` drops only the route.

### 3b. A recorder trait with two impls, not two layers

`HttpMetricsRecorder` (in `recorder.rs`) is the seam:

```rust
pub trait HttpMetricsRecorder: Clone + Send + Sync + 'static {
    fn record_request(&self, method: &'static str, path: &str, status: u16, duration_secs: f64);
    fn inc_in_flight(&self);
    fn dec_in_flight(&self);
}
```

`HttpMetricsLayer<R = PrometheusRecorder>` is generic over it;
`PrometheusLayer` / `PrometheusService` / `PrometheusResponseFuture` remain as
type aliases, so nothing outside the crate had to change. All the interesting
logic — exclusion matching, `MatchedPath` extraction, label bounding, the RAII
in-flight guard, cancellation safety — lives once in the layer and is shared by
both backends. Consequence that matters: **the two stacks emit the same series**
(same names, kinds and labels), so a dashboard survives a stack switch.

Rejected alternatives:

- *Two separate layers, one per stack.* Would duplicate the cancellation-safe
  in-flight guard and the label-bounding rules — the parts that are easy to get
  subtly wrong and that the consumer app's hand-written middleware got wrong.
- *`dyn` recorder.* A trait object per request on the hot path buys nothing;
  the recorder is a zero-sized (prometheus) or pointer-sized (facade) `Copy`.

### 3c. A separate `MetricsFacade` plugin, not `Prometheus<R>`

The facade stack is a *different plugin type* with its own config prefix
(`metrics.*`, keys `namespace` / `exclude_paths` / `enabled`) and
`Provided = ()`. Making `Prometheus` generic over the recorder would have kept
one plugin, but the two do not have the same shape: `Prometheus` provides a
`PrometheusRegistry` bean, owns buckets and an endpoint path; the facade
provides nothing, has no endpoint and cannot own buckets (the exporter does).
A generic plugin would have carried a config section half of which is
meaningless in one instantiation, and a provision list that lies. Two small
plugins with honest surfaces beat one generic plugin with dead knobs.

Choosing a stack is also mutually exclusive in practice, so the ergonomic cost
of two entry points is one line in `main`.

### 3d. Feature flag on `r2e-prometheus`, not a new crate

`metrics-facade` is an optional feature of `r2e-prometheus`
(`metrics-facade = ["dep:metrics"]`), re-exported by the facade as
`r2e/metrics-facade`, and deliberately **not** in `full` — a stack is a choice,
not something to accumulate. A separate `r2e-metrics` crate was rejected: it
would need the layer, the label helpers and the config plumbing, i.e. it would
depend on `r2e-prometheus` or force a third crate to hold the shared parts, for
one 300-line module.

The `prometheus` crate stays a hard dependency of `r2e-prometheus` rather than
being cfg'd out in facade-only builds: it is light, has no side effects until
its registry is touched, and cfg-splitting the crate around it would spread
`#[cfg]` through `lib.rs`/`handler.rs`/`worker.rs` and make `default-features`
handling in the workspace fragile. A facade-only app pays a compile-time
dependency it does not link into any code path it runs; that is the right side
of the trade.

**`metrics-exporter-prometheus` is not a dependency, at any level** — picking
and configuring an exporter is the application's call. It is not even a
dev-dependency: the tests assert against `metrics-util`'s `DebuggingRecorder`,
which is the right tool for "what series did we emit" anyway.

## 4. The facade contract for apps

> Install the R2E layer; own your recorder, your buckets and your endpoint.

```rust
// The app installs its recorder first — descriptions (`# HELP`) are routed to
// whatever recorder is installed at plugin-build time.
let handle = metrics_exporter_prometheus::PrometheusBuilder::new().install_recorder()?;

AppBuilder::new()
    .plugin(MetricsFacade::builder().exclude_path("/health").build())
    // … the app's own `/metrics` route rendering `handle`
```

Emitted series (identical to the `prometheus` backend):

| metric | kind | labels |
|---|---|---|
| `http_requests_total` | counter | `method`, `path`, `status` |
| `http_request_duration_seconds` | histogram | `method`, `path` |
| `http_requests_in_flight` | gauge | — |

`path` is the matched route template or the `unmatched` sentinel; `method` is
one of the nine standard verbs or `other`. With `metrics.namespace: foo` every
name is prefixed `foo_`. Hot-path note: path labels are interned into
`&'static str` (bounded by the route table) and status labels come from a static
table, so recording allocates nothing — `metrics` label values are
`Cow<'static, str>` and an owned one would mean a `String` per metric per
request.

**Migration for an app with a hand-written middleware**: delete the middleware
and the layer wiring, add the plugin, keep the recorder and the `/metrics`
route. Series names match the conventional ones, so dashboards keep working;
check only the `path` label, which becomes the route template where a
hand-rolled middleware often used the raw URI.

## 5. Out of scope (deliberate)

- **Exporting R2E's other metrics through the facade.** `Tenanted`, the
  executor pool, the scheduler and the EventBus backends expose their counters
  as typed collectors / snapshot structs. Routing those through a recorder trait
  is a separate workstream (it needs a metrics *registry* abstraction, not just
  an HTTP recorder), and F13 was about the HTTP stack mismatch.
- **Bucket configuration on the facade path.** The exporter owns bucket layout
  there (`PrometheusBuilder::set_buckets_for_metric`); mirroring it in R2E
  config would be a second source of truth.
- **Auto-detecting the installed recorder.** `metrics` has no "is a recorder
  installed" query that is meaningful before the app's own `main` runs, and
  silently switching stacks is worse than one explicit plugin choice.
- **A `metrics`-facade backend for the `/metrics` endpoint itself.** The facade
  plugin mounts no route by design; rendering an exporter handle is three lines
  in the app and keeps the exporter dependency out of R2E.
