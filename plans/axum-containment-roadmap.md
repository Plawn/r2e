# Roadmap — Contain axum further, without swapping the HTTP backend

Status: **IN PROGRESS** — proposed 2026-10-24, acknowledged by the user the same day (D1–D3). Phase A shipped 2026-10-25 (branch `feat/axum-containment-a-serve`); B and C open. Follow-up to
`plans/runtime-http-dependency-containment.md` (phases 0–7 shipped 2026-08-24).
That plan ended with two written decisions: §5.3d option A ("the public promise is
R2E **types**", raw axum only through `axum_compat`) and §5.3c **DEFERRED** (no
`Router` newtype until a second backend is on the roadmap). This roadmap does not
revisit option A and does not put a second backend on the roadmap. It asks a
narrower question: *what can be contained now so that the day a swap is wanted,
it is a rewrite of `r2e-http`, not of the workspace?*

The trigger for writing it: a comparison with ntex / tako (2026-10-24). Verdict
recorded there: a swap today is a rewrite of the HTTP core and the per-request
gain for a real R2E app (JWT, guards, DI façade, DB) would be marginal. So: no
swap. But the inventory below shows the coupling is not where option A left the
reader thinking it is, and some of it is cheap to move.

## 1. Where we actually are (measured 2026-10-24, `src/` only, tests/benches excluded)

Manifest side is clean: `axum` is a dependency of `r2e-http` only. The *shape*
is not contained:

| coupling | sites | where |
|---|---|---|
| `Router` by name | 38 files; 14 public signatures | `Plugin::configure`, `register_routes`, `merge_router`, `into_router`, `RoutesContext::{register_routes, add_layer, after_routes}`, `AppBuilder::{build, build_with_consumers}`, `Prepared::{router, router_mut}`, `Running::router` |
| `Router` methods actually used | 9 | `route` ×46, `layer` ×34, `with_state` ×7, `merge` ×5, `fallback` ×5, `nest` ×3, `fallback_service` ×3, `route_service` ×1, `method_not_allowed_fallback` ×1 |
| `routing::{get,post,…,any}` | 13 files, 3 codegen sites | `controller_impl.rs` emits `.route(path, routing::<m>(closure).layer(..))` |
| `middleware::from_fn` | 10 files | `#[middleware(fn)]` codegen + plugins |
| `tower::Layer/Service` | 16 files | `with_layer<L: tower::Layer<Route>>`, `with_layer_fn`, `GraphKeepAlive` (a `tower::Service`), `r2e-test` (`ServiceExt::oneshot`), `r2e-grpc` multiplex, `r2e-mcp` `ServiceBuilder` |
| `tower_http::*` | cors ×8, timeout ×5, set_header, normalize_path, catch_panic (doc only) | `r2e-core/builtins`, `runtime/layers.rs`, `secure_headers.rs`, `r2e-mcp/auth`, `r2e-grpc`, `example-app` |
| serve loop | 3 sites | `running.rs:211`, `prepared.rs:912`, `runtime/sharded.rs:409` — each does `into_make_service_with_connect_info` + `ListenerExt` + `axum::serve` |
| hyper semantics relied upon | 3 comments, 1 load-bearing | `prepared.rs:746` (WS upgrade hands the IO over), `layers.rs:376-393` (body split / unknown length), `ws_sessions.rs` |
| `FromRequestParts` / `IntoResponse` | 35 / 25 files | mostly **names** via `r2e_http` re-exports; the *impls* are the 6 named bridge points of plan §5.3b + the `impl_into_response!` macro body + `#[derive(ApiError)]` emission |
| codegen handler shape | 1 | `move |State(__state): State<S>, req| async move { entry(..) }` — already a `(S, Request) -> Response` closure; only `State` and `routing::<m>` are axum-shaped |

What this says: the **extract/response** half is done (plan 3b) and the
**handler** half is one extractor away from done. What is uncontained is the
**routing + middleware + serve** surface, and it is uncontained in both
directions — R2E names it in 14 public signatures, and users hand R2E tower
layers.

## 2. Decisions to take before phase C (the breaking one)

**D1 — tower is a neutral protocol crate, like `http` and `bytes`.** `tower::Layer`
/ `tower::Service` stay nameable workspace-wide and in the public API
(`with_layer`, `#[layer(expr)]`). Reason: it is the ecosystem contract that
hyper, tonic, tower-http and every alternative candidate that sits on hyper
(tako included) speak; abstracting it buys nothing a swap would not have to
redo, and loses users their layers. ntex is the only candidate that would
break this, and ntex is a full-stack swap (runtime + HTTP codec) that this
roadmap explicitly does not prepare for.

**D2 — tower-http is a backend detail, confine it.** Only `r2e-http` (and
`r2e-grpc` for grpc-web CORS over tonic) depend on `tower-http`. Everyone else
reaches the layers through `r2e_http::layers::*`. Re-exports keep type identity,
so nothing user-visible breaks.

**D3 — `Router` leaves the public API (plan §5.3c, re-opened).** Replace it with
an R2E-owned `Routes<S>` carrying the 9 methods in use. This is the breaking
phase and the only one that needs the user's acknowledgement before work starts
(CLAUDE.md § Project Status). `axum_compat` keeps an `into_inner()` /
`from_inner()` pair so the escape hatch still works.

**D4 — out of scope, by decision:** reimplementing extractors (`Path`, `Query`,
`Json` body, matched-path params in extensions), replacing hyper, replacing tower,
HTTP/3 bridge rewrite. These are the swap itself; they wait for a second backend
to actually be on the roadmap, exactly as §5.3c said.

## 3. Phases

Each phase is one PR, independently shippable, with the boundary scripts
tightened in the same PR so it cannot regress. Phases A and B are non-breaking
and can land in any order; C depends on D3 and lands last.

### Phase A — One serve path (non-breaking, small)

- Add `r2e_http::serve::{serve, ServeOptions}`: takes a `Router` (phase C: a
  `Routes`), a `TcpListener`, the shutdown future and the connect-info choice;
  owns `ListenerExt` tweaks and `into_make_service_with_connect_info`.
- Rewrite the three call sites (`running.rs`, `prepared.rs`, `sharded.rs`) on
  top of it. Behaviour identical; the WS-upgrade / drain comments in
  `prepared.rs:746` and `ws_sessions.rs` move next to the code they describe.
- **Perf lever, same PR or the next:** drive hyper through
  `hyper_util::server::conn::auto::Builder` instead of `axum::serve`, which
  exposes the HTTP/1 pipeline-flush and HTTP/2 window settings under
  `server.http.*` config. This is the cheap part of what the ntex/tako
  comparison was really about and it is only possible once the serve path
  is in one place.
- Boundary: `check-source-boundary.sh` gains a group `axum-serve`
  (`axum::serve|into_make_service|ListenerExt`) with allowlist `r2e-http/src/`.
- Exit: `cargo test --workspace` green; sharded + plain + prepared serve paths
  covered by the existing `r2e-core/tests/serve` target; `rg 'http::serve\('`
  outside `r2e-http` returns nothing.

### Phase B — tower-http behind `r2e_http::layers` (non-breaking, small)

- New module `r2e_http::layers` re-exporting `cors`, `timeout`,
  `set_header`, `normalize_path` from tower-http (type identity preserved, so
  `Cors::custom(CorsLayer)` and `with_grpc_web_cors(CorsLayer)` keep their
  signatures).
- Move `default_cors()`, `GraphKeepAlive`, `catch_panic_layer_with` and the
  `NormalizePathLayer` wrap from `r2e-core/src/runtime/layers.rs` into
  `r2e-http` where the backend body semantics they comment on live.
- Migrate the 16 tower-http sites in `r2e-core`, `r2e-mcp`, `example-app`,
  the `r2e-cli` templates and `llm/*.md` snippets.
- Boundary: `check-dep-boundary.sh` adds `tower-http` with allowlist
  `{r2e-http, r2e-grpc}`; `check-source-boundary.sh` adds group `tower_http::`
  with the same allowlist. `tower` itself is **not** added (D1) — write that
  sentence in `docs/claude/architecture.md` so nobody opens a PR to contain it.
- Exit: boundaries green; `llm-doctests` green.

### Phase C — `Routes<S>` replaces `Router` in the public API (breaking, medium)

Needs D3 acknowledged. Bumps `r2e-core` minor and propagates per CLAUDE.md
(facade `r2e`, `r2e-cli`, every crate re-exporting core). Ships with a
`Breaking` changelog entry and `docs/migration/routes-newtype.md`.

- **C1 — the type.** `r2e_http::Routes<S = ()>` wrapping `axum::Router<S>` with
  exactly: `new`, `route(path, Method set, handler)`, `layer`, `route_layer`,
  `with_state`, `merge`, `nest`, `fallback`, `fallback_service`,
  `route_service`, `method_not_allowed_fallback`, plus `into_make_service*`
  used only by phase A's serve. `handler` is R2E's shape: an
  `async fn(S, Request) -> Response` (what the codegen already emits), so
  `State<S>` and `routing::<m>` disappear from the macros. `axum_compat`
  exposes `Routes::into_inner()` / `from_inner()` for `merge_router` users.
- **C2 — codegen.** `controller_impl.rs` emits `Routes::route(path,
  Methods::GET, closure)`; `#[middleware(fn)]` emits `Routes::middleware_fn`
  (wraps `from_fn` inside `r2e-http`); `#[layer(expr)]` stays a tower layer
  (D1). `r2e-compile-tests` snapshots updated.
- **C3 — the 14 signatures.** `Plugin::configure`, `RoutesContext`,
  `register_routes`, `merge_router` (now takes `Routes`, documented as the
  `axum_compat` rung), `into_router` → `into_routes`, `Prepared/Running::router`
  → `routes`. Then every consumer: `r2e-static`, `r2e-grpc` multiplex,
  `r2e-test` (`oneshot` over `Routes`), `r2e-mcp`, `r2e-openapi`, `r2e-oidc`,
  `r2e-prometheus`, `r2e-cli` templates, `example-app`, `llm/*.md`.
- **C4 — close the door.** Remove `Router`, `routing::*`, `middleware::*`,
  `MethodRouter`, `Route` from `r2e_core::http` and the prelude (reachable only
  via `axum_compat`). Boundary: `check-source-boundary.sh` group `axum-shape`
  (`\bRouter\b|routing::|middleware::from_fn|MethodRouter`) with allowlist
  `r2e-http/src/` + the `axum_compat` module.
- Exit: the only crate that can name `Router` is `r2e-http`; the surviving
  backend contracts are the plan §5.3b bridge list + `Routes`'s wrapper body +
  `impl_into_response!` + the `ApiError` derive emission, all tabulated in
  `docs/claude/architecture.md`.

### Phase D — Deferred (unchanged from plan §5.3c)

Handler/extractor contract (`Path`/`Query`/`Json` body extraction, matched-path
params), hyper replacement, HTTP/3 bridge. Trigger to re-open: a concrete second
backend on the roadmap, with a benchmark of `example-app` under `oha`/`wrk`
showing the backend share of per-request time is worth it.

## 4. Sequencing and cost

| phase | breaking | PRs | files touched (approx.) | value |
|---|---|---|---|---|
| A | no | 1 (+1 for hyper tuning) | ~6 | one serve path, unlocks hyper tuning = the real perf lever today |
| B | no | 1 | ~20 | tower-http gone from 5 crates; boundary enforced |
| C | **yes** (core minor → propagated) | 1 lineage PR, C1–C4 as commits or stacked PRs | ~60 + templates + llm docs | `Router` gone from the public API; a swap becomes "rewrite `r2e-http`" |
| D | — | — | — | not now |

Recommended order: **A → B → C**. A and B can be done without any decision;
they also shrink C's diff because the serve and layer code C would otherwise
have to move is already in `r2e-http`.

## 5. Definition of done (this roadmap)

- `scripts/check-dep-boundary.sh`: `axum` = `{r2e-http}`, `tower-http` =
  `{r2e-http, r2e-grpc}`, `tower` unconstrained (D1, written down).
- `scripts/check-source-boundary.sh`: groups `axum`, `axum-serve`,
  `axum-shape`, `tower_http` all confined to `r2e-http/src/`.
- `docs/claude/architecture.md` § Containment Boundaries lists the complete
  remaining backend surface: `Routes` wrapper body, `serve`, `layers`, the 6
  bridge points, `impl_into_response!`, `ApiError` emission, `axum_compat`.
- `llm.txt` / `llm/*.md`: no snippet names `Router`, `routing::`, `tower_http`.
- `docs/claude/roadmap.md` W11 "Zero-copy exploration (xitca-web)" bullet is
  replaced by a pointer to this file and to phase D's trigger.

## 6. Execution log

- **2026-10-25 — Phase A shipped.** `r2e_http::server::{serve, ServeOptions}`;
  `running.rs`, `prepared.rs` (single listener), `runtime/sharded.rs` and
  `r2e-test`'s `TestServer::new` rewritten on it. `r2e-rt` + `tracing` became
  unconditional deps of `r2e-http`. Boundary: new source group `axum-serve`
  (`into_make_service*|with_graceful_shutdown|tap_io|ListenerExt`); its
  baseline is `server.rs` itself plus the two `ListenerExt` re-exports
  (`r2e-http/src/lib.rs`, `r2e-core/src/http/mod.rs`) that phase C4 removes.
  The `axum` baseline gains `r2e-http/src/server.rs` (reviewed exception,
  recorded in the baseline header). Deferred from A to its own PR: driving
  hyper through `hyper_util` `auto::Builder` for `server.http.*` tuning — the
  serve path is now in one place, so that change is contained to `server.rs`.
