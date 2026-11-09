# Migration: layer bands — `LayerSlot` (r2e-core 0.9 → 0.10)

Until 0.9 the position of an HTTP layer in the stack was the position of its
`.plugin(..)` call: Graph-stage layers were applied in install order, the
later one outside. The stack was therefore a property of each application's
`main`, and getting it wrong was silent — a gate installed after `Cors`
answered `401` without CORS headers (the browser reports a network error
instead of the status), a `Prometheus` installed first never saw a preflight,
a trace installed too early logged nothing for what the outer layers rejected.

From 0.10 a layer declares the **band** it belongs to and the band decides
its position. From the outside in:

```text
Trace  >  Metrics  >  Cors  >  Auth  >  App  >  routes
```

| Band | What goes there | Built-ins |
|---|---|---|
| `LayerSlot::Trace` | request id, request span, access log | `HttpTrace`, `RequestIdPlugin`, `Observability` |
| `LayerSlot::Metrics` | HTTP metrics | `Prometheus`, `MetricsFacade` |
| `LayerSlot::Cors` | CORS and response-header policy | `Cors`, `SecureHeaders` |
| `LayerSlot::Auth` | a layer that rejects before a handler runs | — (yours) |
| `LayerSlot::App` | everything else — the default | `add_layer`, `with_layer`, `with_layer_fn` |

Inside one band the old rule still holds: install order, later = outer.

**Nothing fails to compile** for applications and plugins that go through
`PluginBuildContext::add_layer` and the builder — this is a *behaviour*
change, which is why it ships in a minor bump and why the checklist below is
worth five minutes per application.

## What changes

| Situation in 0.9 | In 0.10 |
|---|---|
| `.plugin(HttpTrace)` had to come **last** to be outermost | any position; always outermost |
| `.plugin(Prometheus)` first ⇒ innermost: preflights and gate rejections were not counted | sits right inside the trace: `OPTIONS` preflights and `401`/`403` from a gate **are** counted (`http_requests_total` goes up accordingly) |
| `.plugin(Cors)` had to come after any gate plugin, or the gate's `401` had no CORS headers | `Cors` is always outside the `Auth` and `App` bands |
| `SecureHeaders` position followed its install | `Cors` band: the headers are on preflight answers and gate rejections too |
| a plugin's `ctx.add_layer(..)` position followed its install, relative to **every** other layer | `App` band — innermost; install order only relative to other `App` layers |
| `AppBuilder::with_layer(l)` / `with_layer_fn(f)` (called after `build_state()`) were **outermost**, outside trace and CORS | `App` band — **innermost**, inside every plugin layer of the four outer bands |
| a `with_layer_fn(\|r\| r.nest("/ui", ui))` added routes that no plugin layer wrapped | those routes are wrapped by the `Trace`, `Metrics`, `Cors` and `Auth` bands (not by earlier `App` layers) |

Unchanged: per-route `#[layer(..)]`, guards and interceptors, `NormalizePath`,
the two catch-panic layers, `wrap_router` (Finalize) and its ordering.

## What to do

1. **A gate written as a plugin** (API key, OAuth, tenant allow-list — anything
   that returns early):

   ```rust
   // before
   ctx.add_layer(move |routes| routes.middleware_fn_with_state(gate, authenticate));
   // after
   ctx.add_layer_in(LayerSlot::Auth, move |routes| {
       routes.middleware_fn_with_state(gate, authenticate)
   });
   ```

   Left on `add_layer` it still works, in the `App` band — inside CORS, trace
   and metrics, which is already the safe side. Declare `Auth` so it also
   stays outside the application's own `App` layers.

2. **A hand-built CORS layer on the builder** — it used to be outermost, it
   is now innermost, *inside* any gate. Give it its band (or switch to the
   `Cors` plugin):

   ```rust
   // before
   app.with_layer(CorsLayer::new().allow_origin(Any))
   // after
   app.with_layer_in(LayerSlot::Cors, CorsLayer::new().allow_origin(Any))
   ```

3. **Any other `with_layer` / `with_layer_fn` that relied on being outside
   the plugins** (a global timeout that must cover the trace, a body-limit
   that must run before the gate): pick the band with `with_layer_in` /
   `with_layer_fn_in`. An `Extension(..)` layer needs nothing.

4. **Routes added through `with_layer_fn`** (`nest`/`merge`): check that they
   are fine behind your `Auth` gate. If they must stay public, exempt the
   path in the gate, or mount them with `merge_router` and handle them as any
   other public route.

5. **Dashboards and alerts on `http_requests_total` / latency histograms**:
   preflights and gate rejections now show up. Filter on `method!="OPTIONS"`
   or on the status label if a panel should keep its old meaning.

6. **Delete ordering comments and helpers.** `// must be installed last`,
   `http_stack!`-style macros whose only job was to fix an order: the order
   is no longer yours to maintain.

## Low-level API

Only relevant if you build a `DeferredContext` by hand (tests of a plugin's
`after_build` closure):

| Before | After |
|---|---|
| `DeferredContext.layers: &mut Vec<Box<dyn FnOnce(Routes) -> Routes + Send>>` (`#[doc(hidden)]`) | `&mut Vec<(LayerSlot, Box<dyn FnOnce(Routes) -> Routes + Send>)>` |
| `dctx.add_layer(Box::new(f))` | unchanged (`App` band); `dctx.add_layer_in(slot, Box::new(f))` for a band |

## Why a closed list

`LayerSlot` is `#[non_exhaustive]` but deliberately has no numeric priority
and no "before/after plugin X" form: either would hand the ordering problem
back to each application. If a layer fits none of `Trace`/`Metrics`/`Cors`/
`Auth`, it is an `App` layer; if two `App` layers need a fixed relative
order, that is install order, as before.
