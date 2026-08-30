# Hot-path clone audit — per-request deep clones of immutable data

Date: **2026-08-29** — task #990 acceptance criterion "framework middleware /
per-request paths that implicitly deep-clone immutable configuration or other
immutable structures per request"; the regression guard, the before/after
measurements and the global-lifetime review below landed under task #982.

Precedent: commit `75495d5` *perf(prometheus): share immutable layer config* —
`PrometheusLayer`/`PrometheusService` held `MetricsConfig` by value; it became
`Arc<MetricsConfig>`, wrapped once in `PrometheusLayer::new`. That is the shape
every fix below follows.

## The invariant

> **Configuration is `Arc`'d once at layer/plugin/decorator build time.
> Everything a per-request path clones is an `Arc`, a `Copy`, or genuine
> per-request data.**

## Why a "layer field" is a per-request clone

Two mechanisms in the HTTP backend turn a struct field into a per-request cost.
Both were verified in the vendored `axum-0.8.9` source rather than assumed:

1. **The service stack is cloned per request.**
   `axum/src/routing/route.rs:51` — `Route::oneshot_inner` does
   `self.0.clone().oneshot(req)`. `self.0` is the boxed, fully layered service,
   so cloning it clones *every* `tower::Service` struct in the stack. A
   `Vec<String>` or a config struct held by value in a layer's service is
   therefore deep-copied on every single request.

2. **The router state is cloned per request.**
   `axum/src/handler/service.rs:168` — `Handler::call(handler, req,
   self.state.clone())`, unconditionally, whether or not the handler declares
   `State<S>`. R2E installs the resolved bean HList as that state
   (`r2e-core/src/builder/typed.rs`, `router.with_state(state.clone())`) —
   since task #992 behind one `Arc` (`BeanState`), so that clone is a single
   refcount bump whatever the graph's width; see "The router state is one
   `Arc`" below.

Everything else — decorator sets, controller cores, route metadata — is built
once at registration and reached through an `Arc`, so it is out of the blast
radius by construction.

## Method

- Enumerated every `tower::Layer`/`Service` impl, `middleware::from_fn`
  closure, router fallback closure, `FromRequestParts` impl and generated
  handler body in scope.
- Scanned those files for `.clone()` / `.to_string()` / `.to_owned()` /
  `.to_vec()` / `collect()` and classified each hit as registration-time,
  per-request-on-immutable-data, or per-request-on-request-data.
- Cross-read the emitted `quote!` templates in `r2e-macros/src/codegen` rather
  than only the macro code, to separate macro-expansion-time allocation
  (harmless) from allocation inside the emitted async handler body.
- Cheap clones (`Arc`, `Copy`, `HeaderValue`, `Bytes`, `MatchedPath`) are not
  findings.

Crates in scope: `r2e-core` (http/plugin/runtime/controller/decorators/web),
`r2e-http`, `r2e-macros` (codegen), `r2e-utils`, `r2e-security`,
`r2e-rate-limit`, `r2e-prometheus`, `r2e-observability`, `r2e-tenant`,
`r2e-static`, `r2e-mcp`, `r2e-grpc`, `r2e-openapi`.

## Findings

| Crate | Site | What is cloned | Per-request? | Verdict |
|---|---|---|---|---|
| `r2e-prometheus` | `src/layer.rs:42,68` | `MetricsConfig` (exclude-path `Vec<String>` + buckets) | yes — service stack cloned per request | **fixed** (`75495d5`, precedent) — `Arc<MetricsConfig>` |
| `r2e-observability` | `src/middleware.rs:38,55,64` | `capture_headers: Vec<String>` deep-cloned in `Layer::layer` and in every `OtelTraceService::clone` | yes — same mechanism | **fixed** — `Arc<[String]>`, built once in `OtelTraceLayer::new` |
| `r2e-openapi` | `src/handlers.rs:19,34` | the entire rendered OpenAPI document (`String`) re-allocated and memcpy'd on every `/openapi.json` hit | yes — inside the handler closure | **fixed** — pre-encoded once as `Bytes`; the per-request clone is now a refcount bump |
| `r2e-utils` | `src/interceptors.rs` `CacheInvalidateInterceptor::around` | `self.group.clone()` (`String`) **plus** `format!("{}:", group)` — both constant for the interceptor's life | yes — once per intercepted request | **fixed** — the `"group:"` prefix is built once in `DecoratorSpec::build` and held as `Arc<str>`; per request it is an `Arc::clone` |
| `r2e-utils` | `src/interceptors.rs` `Counted::around`, `MetricTimed::around` | `self.metric_name.clone()` (`String`), set once at construction | yes | **fixed** — borrowed (`as_str()`); the RPITIT future already captures `&self` and is awaited in place |
| `r2e-utils` | `src/interceptors.rs` `Timed`/`Counted`/`MetricTimed` | `&format!(..)` message allocated before `tracing` decides whether the level is enabled | yes | **fixed** — new private `log_args_at_level` takes `format_args!`, so nothing is allocated and a filtered-out level formats nothing. `log_at_level` (public) keeps its signature and delegates |
| `r2e-utils` | `src/interceptors.rs` `Cache::full_key` | two `String` allocations to build one cache key | yes | **fixed (partial)** — collapsed to a single `format!`. It cannot be precomputed: `#[intercept]` on an impl block builds **one** interceptor shared by every route, so the key depends on the `&'static str` method name passed in per call |
| `r2e-prometheus` | `src/metrics.rs` `record_request` | `status.to_string()` — the only heap allocation left on the metrics path (`with_label_values` itself just hashes) | yes | **fixed** — rendered into a stack buffer by `status_label` |
| `r2e-core` | `src/builder/typed.rs` `router.with_state(state)` | the resolved bean HList — one `Clone` per bean, per request | yes — the backend clones the router state unconditionally | **fixed** (task #992) — the list is held behind one `Arc` (`BeanState<L>`); the per-request clone is a single refcount bump and no bean's `Clone` runs at all. See "The router state is one `Arc`" below |
| `r2e-core` | `src/builtins/request_id.rs:84` | `s.to_string()` of the incoming `X-Request-Id` | yes | acceptable — genuine per-request data; the `HeaderValue` beside it is already reused instead of re-parsed, and the generated path writes a UUID into a stack buffer |
| `r2e-core` | `src/builtins/secure_headers.rs:70-78` | `Vec<(HeaderName, HeaderValue)>` | no — moved into `SetResponseHeaderLayer`s at plugin build | acceptable |
| `r2e-core` | `src/runtime/layers.rs` `GraphKeepAlive`, `normalize_path_router`, `catch_panic_layer`, `default_cors` | `Arc<BeanContext>` only | yes, but `Arc` | acceptable — deliberate, documented |
| `r2e-core` | `src/decorators/*` | *nothing* — `GuardContext`/`InterceptorContext` are `&'a` borrows over `&'static str`; zero `.clone()` in the whole directory | n/a | clean |
| `r2e-core` | `src/web/{params,multipart,validation,ws,sse}.rs` | query/multipart/WS payload `to_string()`/`to_vec()` | yes | acceptable — request payload, not app-lifetime data. Room broadcasters clone channel handles, not buffers |
| `r2e-core` | `src/config/mod.rs:158` | `R2eConfig` | cloned with the state per request | acceptable — already `Arc<HashMap<..>>` |
| `r2e-core` | `src/builtins/health.rs` | indicator names `to_string()`, response `clone()` | per health probe only, and the response is TTL-cached | acceptable |
| `r2e-http` | `src/quic.rs:401` `apply_alt_svc` | `HeaderValue::clone()` per request | yes | acceptable — `Bytes`-backed refcount bump |
| `r2e-http` | `src/labels.rs` | `method_label`/`route_label` | yes | clean — both return `&str`, no allocation, deliberately shared by both telemetry backends |
| `r2e-http` | `src/json.rs` | `to_string()`/`clone()` | error paths only | clean |
| `r2e-static` | `src/lib.rs:533,567` | `req.uri().path().to_string()` and the per-request `mime_guess(..).to_string()` | yes | acceptable — derived from the request path, not app config. The handler and its `StaticConfig` are `Arc`'d once and read by reference |
| `r2e-rate-limit` | `src/guard.rs:529,588` | `format!("{controller}:{method}")` scope + a second `format!` for the bucket key | yes | deferred — see below |
| `r2e-macros` | `codegen/{handlers,decorators,controller_codegen,controller_impl,wrapping,transverse}.rs` | route metadata `to_string()`/`vec![]`, `Arc::new`/`Arc::clone` of core + deco sets | **no** — all inside `register_controller`/`register_meta`, not the emitted async body | clean. The façade reaches `#[inject]`/`#[config]` fields through `Deref` to the shared core, so a `#[config("x")] String` is never cloned per request |
| `r2e-macros` | `codegen/handlers.rs:775,780,1097,1102,1482,1539` | `HeaderMap` and `Extensions` extracted **by value** into the generated head group | yes, but only on routes that have guards or `#[managed]` params | deferred — see below |
| `r2e-security` | `src/jwt.rs:181` | `jsonwebtoken::Validation` — 3 `HashSet<String>` + `Vec<Algorithm>` + several `String`s — cloned *just to overwrite one field* | yes — `validate_as` is the single validation entry point for **all three transports** (HTTP `AuthenticatedUser`, gRPC `extract_jwt_claims_from_metadata`, MCP `validate_jwt`) | **fixed** — one `Validation` per allowed algorithm is built at construction (`build_validations`); the request path borrows the matching entry |
| `r2e-security` | `src/jwks.rs:209` | `format!("{algorithm:?}")` to compare against the JWK's advertised `alg` | yes — `validate_key_metadata` runs on every validation whose JWK advertises `alg` (Keycloak, Auth0, Entra all do) | **fixed** — `name_for_algorithm(alg) -> &'static str` |
| `r2e-mcp` | `src/auth/opaque.rs:102` | the cached `McpPrincipal` (whole `AuthenticatedUser` + `StandardClaims` incl. the flattened `extra` JSON map) deep-cloned **while the cache `Mutex` is held** | yes — the fast path of the opaque-token (introspection / userinfo) backends | **fixed** — `CacheEntry.result` is `Result<Arc<McpPrincipal>, _>`; the lock is now released after an `Arc` bump and the principal is materialized outside the critical section |
| `r2e-mcp` | `src/auth/wellknown.rs:53` | the prebuilt PRM document `Arc<str>` re-`to_string()`d per `/.well-known/oauth-protected-resource*` GET | yes | **fixed** — encoded to `Bytes` once in `prm_routes`; `public_json_response` now takes `Bytes` |
| `r2e-mcp` | `src/auth/shim.rs:235` | the mirrored AS-metadata document copied three times per GET (`String` → `Arc<str>` → `String`) | yes | **fixed (partial)** — down to one copy (`String` → `Bytes`). Memoizing the render per discovery generation is deferred, below |
| `r2e-mcp` | `src/auth/layer.rs:174` | `principal.user.clone()` — the same `AuthenticatedUser` inserted twice so both extensions exist | yes | **fixed** — `McpPrincipal.user` is an `Arc<AuthenticatedUser>` and the layer deposits `Arc::clone` of it, so an authenticated `tools/call` costs **200 allocations / 32,976 bytes** whatever the token carries, instead of 764 / 129,176 with a 32 KiB claims tree |
| `r2e-mcp` | `src/handler.rs:121,182` | `Vec<Tool>` / `Vec<Resource>` / `Vec<Prompt>` / `Vec<ResourceTemplate>` cloned per `*/list` | yes | **fixed** (tools) — `ToolRoute::description` is `Cow<'static, str>` and `#[tool]` emits a borrowed literal, so `tools/list` clones **1 allocation / 1056 bytes** for 6 tools instead of 7 / 1080 (7 / 2748 with long descriptions). The lists themselves were already built once at boot and filtered before cloning; `Resource`/`Prompt`/`ResourceTemplate` still copy their `String`s — rmcp models them as `String` |
| `r2e-mcp` | `src/auth/layer.rs` `AuthState`, `auth/validator.rs`, `auth/discovery.rs` | `Arc` / `Arc<str>` / `Arc<[String]>` only; scope checks borrow `&StandardClaims` | yes, but `Arc` | clean |
| `r2e-mcp` | `src/uri_template.rs:243-296` | template variable names copied into the per-call `BTreeMap<String, String>` | yes, per `resources/read` | acceptable — forced by the public `ResourceCall::variables` type |
| `r2e-grpc` | `src/multiplex.rs` `call()` | `self.grpc.clone()` / `self.http.clone()` | yes | clean — `tonic::service::Routes` wraps `axum::Router`, whose `Clone` is an `Arc` bump (verified in `tonic-0.14.6/src/service/router.rs:14`, `axum-0.8.9/src/routing/mod.rs:72`) |
| `r2e-grpc` | `src/guard.rs`, `src/identity.rs` | — | yes | clean — `GrpcGuardContext` borrows the metadata map, bearer extraction returns `&str`, roles are `&'static [&'static str]` |
| `r2e-tenant` | `src/{extract,id,router,resolver,plugin,map/*}.rs` | `TenantId(Arc<str>)`, `TenantRouter = Arc<Mode>`, `Tenanted<T>` = `Arc` inner, `TenantedSettings: Copy`, resolvers hold `Cow<'static, str>` and borrow it | yes, but `Arc`/`Copy` | clean — the whole crate is already built to this invariant |
| `r2e-tenant` | `src/source.rs:195` | `ResolutionChain::root::<T>()` allocates a 1-element `Vec` on every `Tenanted::get()`, including cache hits, though it is only read when a `create` runs | yes | acceptable — one small `Vec`, not a clone of shared data; could be made lazy

## The regression guard

`examples/example-app/tests/hotpath/` is a plain `cargo test` target (no
criterion, no nightly bench) that makes per-request allocations visible and
fails CI when they come back:

```bash
cargo test -p example-app --test hotpath                    # the guard
cargo test -p example-app --test hotpath -- --nocapture     # + the numbers
cargo test -p r2e-mcp     --test hotpath                    # the MCP guards
```

`r2e-mcp/tests/hotpath/` is the same harness (its `counter.rs` is a copy — a
`#[global_allocator]` can only be installed by the binary that measures with
it) for the two MCP hot paths, `*/list` payloads and the authenticated-request
principal.

It lives in `example-app` because that is the only workspace member that
depends on `r2e-prometheus`, `r2e-observability`, `r2e-security`, `r2e-utils`
and `r2e-openapi` at once — `r2e-core` sits *below* all of them, so the
wrappers cannot meet in a core test.

- `counter.rs` installs a counting `#[global_allocator]` (test target only,
  never in library code). The counters are `thread_local!` + `const`-init
  `Cell<u64>`: no `Drop`, no lazy init, so the allocator cannot re-enter
  itself, and a parallel `cargo test` thread cannot contaminate another's
  count. Every measurement runs on a `current_thread` runtime for the same
  reason.
- `steady_state(n, f)` runs `f` `n` times to warm up (first-touch faults, lazy
  metric registration, JWKS/validation caches), then measures `n` more and
  divides. `measure` is the one-shot form.

**What it asserts is mostly config-size invariance, not an absolute budget.**
Each invariance test builds the same wrapper twice — once with a small
immutable config, once with a large one — and asserts the per-request cost does
not grow. That is exactly the bug class this ticket is about (an app-lifetime
`Vec<String>`/`String`/`Validation` copied per request), and it is the *shape*
of the assertion, not a baseline, so a different CPU or a dependency bump moves
both sides together and the test still holds.

Two absolute numbers do appear, and both are slack, not budgets:

- `assert_config_size_invariant` takes a `slack_count`/`slack_bytes` (2–4
  allocations, 256–512 bytes) so incidental jitter — a differently-sized
  `format!` buffer, one extra small `Vec` — does not fail the run. Every call
  site sizes its large config so a real deep clone overshoots the slack by an
  order of magnitude (≥ 64 allocations / ≥ 4 KiB), which is what keeps the
  slack from hiding the bug it guards. A dependency bump that adds a few
  allocations to *both* sides is invisible to it; one that adds them to the
  large side only is the regression.
- `layers::composed_stack_budget` (120 allocations / 16 KiB per request) is an
  outright absolute canary for an order-of-magnitude regression, and it *is*
  machine- and dependency-specific in principle (it currently measures 23 / 1981
  on the machine below, so it has ~5× of headroom). It prints its figure, so it
  can be re-baselined from the test output.

The counting allocator charges an event per `alloc`/`alloc_zeroed`/`realloc`
and the *full* requested size — for a `realloc`, `new_size`, not the growth
delta, since that is the request the allocator receives and the block it may
have to move. `accounting.rs` pins that rule (growth, shrink, repeated
doubling, `dealloc` is free, thread-locality).

Coverage: the Prometheus layer and the OpenTelemetry trace layer
(`layers.rs`), the built-in interceptors `Logged`/`Timed`/`Counted`/
`MetricTimed` (`decorators.rs` — with no subscriber installed every level is
disabled, so a correctly-lazy interceptor must allocate *nothing*),
`JwtClaimsValidator::validate_as` (`jwt.rs`), `GET /openapi.json`
(`openapi.rs`), and the router state itself (`state.rs` — width invariance
rather than config-size invariance: the per-request cost must not grow with the
number of beans in the graph).

The guard was validated against the real regression: with the seven hot-path
sources reverted to their pre-fix state, six of the seven tests fail (see the
table below).

## Before/after

**Machine.** Apple M4 (10 cores), macOS 15.7.7, rustc 1.100.0-nightly
(bff8e12ff 2026-08-26), `--release`, all on localhost (loopback, so the numbers
are a framework comparison, not a capacity figure).

**"Before" = `d046b84`**, the merge immediately preceding `0443f28`, the first
hot-path commit. `BEFORE_REV` makes the script do the reconstruction itself: it
rewinds the seven hot-path sources to that revision and restores them from an
EXIT trap, so an interrupted run does not leave the tree rewritten. Only those
seven move — plenty of other files differ between `d046b84` and this branch's
HEAD (the worker runtime, MCP auth, …), and leaving them at HEAD is what makes
the delta this workstream's change rather than the branch's. The app, its
config, the load generator and its parameters are identical across the two
runs. The script **refuses to start on a dirty tree**, because the restore is a
`git restore --source=HEAD` over those paths and would discard local edits to
them:

```bash
git status --porcelain        # must be empty
LABEL=before DURATION=10s CONNS=64 BEFORE_REV=d046b84 tools/bench-hotpath.sh
LABEL=after  DURATION=10s CONNS=64                    tools/bench-hotpath.sh
```

The run refuses to measure a server it cannot prove is its own: the port must
be free beforehand, the launched PID must own it and still be alive, the app
carries a unique per-run marker read back from `GET /config`, and every
endpoint must answer 2xx both in a pre-flight probe and across the whole `oha`
status distribution.

`tools/bench-hotpath.sh` builds `example-app` in release, boots it, scrapes the
demo JWT it prints, runs a 3 s warm-up then a 10 s `oha` run at 64 connections
against three endpoints, and prints the markdown rows below.

### HTTP load

| label | endpoint | wrappers exercised | req/s | p50 | p99 |
|---|---|---|---|---|---|
| before | `/mixed/public` | prometheus + otel layers + handler | 27998 | 2.27 ms | 4.34 ms |
| after | `/mixed/public` | prometheus + otel layers + handler | 29133 | 2.18 ms | 4.19 ms |
| before | `/users/` | + JWT validation + `Logged`/`Timed` | 27005 | 2.35 ms | 4.54 ms |
| after | `/users/` | + JWT validation + `Logged`/`Timed` | 28589 | 2.21 ms | 4.31 ms |
| before | `/openapi.json` | immutable document | 57951 | 1.09 ms | 2.13 ms |
| after | `/openapi.json` | immutable document | 64087 | 0.98 ms | 1.96 ms |

+4.1 % / +5.9 % / +10.6 % throughput, p99 down 3–8 %. Modest, and that is the
honest result: on a loopback benchmark with a *small* configuration the copied
data is small too. The point of the change is the shape of the curve — see
below, where the per-request cost stops depending on the size of the config.

### Per-request allocations

From `cargo test -p example-app --test hotpath -- --nocapture`, run once in
each state. "small" / "large" is the same wrapper with a small vs. a large
immutable config (64 exclude paths / 64 capture headers / 64 extra audiences /
200 routes / a 4 KiB metric name).

The **before** column was captured with the counter's earlier `realloc` rule
(the growth delta rather than the full `new_size`). Re-running the "after"
column under the current rule reproduces it byte for byte — these paths never
`realloc` — so only the before figures could move, and only upward: the gap
below is a lower bound on the one you would measure today.

| Measurement | before, small | before, large | after, small | after, large |
|---|---|---|---|---|
| Prometheus + otel + route (`MetricsConfig` grows) | 44 allocs / 2718 B | 233 allocs / 19137 B | 23 / 1981 B | 23 / 1981 B |
| Prometheus + otel + route (`capture_headers` grows) | 41 / 2709 B | 293 / 24581 B | 23 / 1981 B | 23 / 1981 B |
| `JwtClaimsValidator::validate_as` | 24 / 1955 B | 88 / 9087 B | 13 / 1484 B | 13 / 1484 B |
| `GET /openapi.json` | 17 / 3935 B | 17 / **143697 B** | 16 / 1149 B | 16 / 1151 B |
| `Counted::around` (metric name grows) | 2 / 31 B | 3 / 8207 B | 0 / 0 B | 0 / 0 B |
| `Logged::info` / `Timed` (level disabled) | 0 / 0 B | — | 0 / 0 B | — |
| composed stack, absolute | 47 allocs / 2814 B | — | **23 allocs / 1981 B** | — |

The invariant holds after the fix: every "after" pair is identical (±2 bytes of
`Content-Length` digits on the OpenAPI row) no matter how large the
configuration is, where before the cost grew with it — an app serving a 140 kB
OpenAPI document was memcpy'ing all of it on every `/openapi.json` hit, and an
app with a long exclude-path list was paying one allocation per entry on
*every request to every route*.

## The router state is one `Arc` (task #992)

This was the audit's first Deferred row, and the one whose cost grew with the
app rather than with a config value. It landed under task #992.

### The finding

`r2e-core/src/builder/typed.rs` installs the resolved bean HList as the router
state, and the backend clones that state on **every** request whether or not
the handler declares `State<S>` (`axum/src/handler/service.rs`,
`Handler::call(handler, req, self.state.clone())`). `HCons` derives `Clone`, so
the per-request cost was the sum of every bean's `Clone` — O(N) in the width of
the bean graph. Beans are `Arc`-shaped *by convention*, so that was usually N
refcount bumps rather than N deep copies, but nothing enforced it: one bean
holding a `String` by value made every request in the app pay a heap copy,
invisibly.

### The fix

`BeanState<L>` (`r2e-core/src/type_list.rs`) — the materialized `HCons` chain
held behind a single `Arc`. `build_state()` returns
`AppBuilder<BeanState<<P as BuildHList>::Output>>`, so `BeanState` *is* the
router state; each per-request clone is one refcount bump regardless of N. (The
backend takes two of them per request — see the measurement below — so the
guarantee is O(1) in the number of beans, not "one clone".)

The list itself is unchanged, and so is the cost of reading a bean: every
access trait is forwarded to the inner list with its index witness intact, so
`state.get::<T>()` still monomorphizes to one pointer dereference plus the same
constant field offset — no `TypeId` lookup, no hash, no downcast. The
witness-free `state.bean::<T>()` keeps *its* cost too, which was never a fixed
offset: a runtime `TypeId` walk down the list, O(N) integer compares, now with
one dereference in front. This change neither helps nor hurts it.

| Trait | How it reaches the list |
|---|---|
| `HasBean<T, Idx>` | delegated, `Idx` unchanged — the witnesses generated extractors carry keep working |
| `Contains<H, Idx>` | delegated, so every `AllSatisfied<StateType, _>` bound (controller `Deps`, `register_grpc_service`, MCP service registration, module scope checks) sees through the wrapper |
| `BeanLookup` | delegated — `state.bean::<T>()`, `ManagedResource` providers. Still the runtime `TypeId` walk it always was, not a fixed offset |
| `BeanAccess::get` | free: a blanket impl over any `Self: HasBean<T, Idx>` |
| `Deref<Target = L>` | for the rare code that wants the list itself |

Nothing in `r2e-macros` needed to change: the generated request extractor
`__R2eRequestData_<C><__M>` and the `Controller<S, W>` impl were already
state-generic, bounded on `HasBean` / `BeanLookup` rather than on `HCons`. That
generality is what made the wrapper a drop-in.

### Before/after

`examples/example-app/tests/hotpath/state.rs` — the same router built over a
narrow (8-bean) and a wide (64-bean) state, driven through the same
`current_thread` runtime as the rest of the target. Two bean flavours, because
the two halves of the finding are different: an `Arc`-shaped bean that counts
its own `Clone` calls (refcount traffic no allocation counter can see) and a
`String`-owning bean whose cost the allocation counter does see.

"Before" is reproduced by replacing that file's `StateOf`/`into_state` with the
identity (`type StateOf<L> = L`), which is exactly the pre-#992 state shape;
everything else in the file is unchanged.

| Measurement (per request) | before, 8 beans | before, 64 beans | after, 8 beans | after, 64 beans |
|---|---|---|---|---|
| bean `Clone` calls (`Arc`-shaped beans) | 16 | **128** | 0 | **0** |
| allocations (beans owning a `String`) | 30 allocs / 2213 B | **142 allocs / 10389 B** | 14 / 1053 B | **14 / 1053 B** |

Two observations:

- The backend clones the state **twice** per request, not once — 16 and 128
  clones for 8 and 64 beans. So the old cost was 2N bean clones per request on
  every route, and the fix removes all of them: a bean's `Clone` no longer runs
  on the request path at all.
- The owning-bean row is the reason this was worth doing even though beans are
  `Arc`-shaped by convention. 64 such beans cost 112 extra allocations and
  ~8 KiB per request before; after, the bean's shape stops mattering, and the
  absolute figure drops below the `Arc` case's too (the wrapper removes the
  `HCons` chain copy itself, which the allocator was seeing as part of the
  boxed-service clone).

The same two properties are asserted a second time against the router
`AppBuilder::build_state().…​.build()` actually produces (`provide` → `build_state`
→ `build_inner`'s `with_state`), so unwrapping the state anywhere on that path
fails the guard even while `BeanState` itself stays correct. That router carries
the framework's own layers, so its "before" figures are higher again — 32 and
256 bean clones per request at 8 and 64 beans, i.e. four state clones rather
than two. The "after" figure is 0 at both widths, as above.

### The rest of the guard set

- `r2e-core/tests/controller/scope.rs::bean_backed_request_extractor_resolves_through_the_state`
  — the positive request-path case: a macro-generated `#[inject(request)]`
  extractor written like `AuthenticatedUser` (`FromRequestPartsVia` + `ViaBean`)
  pulls its bean out of the wrapper at request time, index witness threaded
  through `__R2eRequestData_*`. The compile-time complement (exactly one
  extraction route) is `tests/http/extract.rs`.
- `r2e-core/tests/runtime/dev_reload.rs::a_full_cache_hit_reuses_the_same_state_arc`
  (feature `dev-reload`) — a full cache hit hands back the *same* `Arc`:
  `std::ptr::eq` on `BeanState::list()`, plus a counting-`Clone` bean proving no
  bean was re-cloned out of the context into a fresh list.

No HTTP-load row: the delta is invisible on `example-app`, which has ~10 beans
and measures 2×10 refcount bumps against ~35 µs of request handling. The
property this change buys is the shape of the curve — flat in N instead of
linear — which is what the table above measures.

## MCP `tools/list` is borrowed metadata (task #994)

### The finding

`Family::visible_list` hands every `tools/list` a clone of the `Vec<Tool>`
built at boot, and `Family::wire` clones one element per `tools/call`. Clients
re-list (and `list_changed` forces them to), so this is a hot path, not a
once-per-session cost.

The list *itself* was already right: `Family::build` precomputes it, and the
`mcp.auth.filter-members` visibility filter runs over `(wire, requirements)`
pairs **before** cloning, so a filtered list is built from the prebuilt
elements rather than rebuilt from the routes. What was wrong was the element:
rmcp's `Tool` stores `name` and `description` as `Cow<'static, str>`, but
`ToolRoute::description` was an `Option<String>`, so `to_rmcp_tool` produced a
`Cow::Owned` and every clone re-allocated a string the macro had emitted as a
literal.

### The fix

- `ToolRoute::description` is `Option<Cow<'static, str>>`. **Breaking** for
  hand-built routes: `description: Some("…".into())` still compiles,
  `Some(String)` needs `.into()`.
- `#[tool]` emits `Cow::Borrowed` (`opt_cow_str` in
  `r2e-macros/src/mcp_codegen/service_impl.rs`), so every macro-declared tool
  is borrowed end to end.
- `to_rmcp_tool` uses `Tool::new_with_raw`, which takes the description
  as-is instead of round-tripping it through `unwrap_or_default()`.

`ToolRoute::name` was already `Cow<'static, str>`; `title` stays `String`
because rmcp's `Tool::title` is a `String` and would re-allocate on
conversion anyway (a tool that sets `title` therefore still costs one
allocation per request — the macro sets it only when `#[tool(title = "…")]`
is given).

### Before/after

Per `tools/list` clone of a six-tool service (input + output schemas,
annotations, doc-comment descriptions), measured by
`r2e-mcp/tests/hotpath/lists.rs`:

| descriptions | before | after |
|---|---|---|
| short (`"Add."`) | 7 allocations / 1080 bytes | **1 allocation / 1056 bytes** |
| long (~280 chars) | 7 allocations / 2748 bytes | **1 allocation / 1056 bytes** |

1056 bytes is `6 * size_of::<Tool>()` — the destination `Vec` and nothing else.
The cost is now flat in what the tools say about themselves.

### What is still copied

`Resource`, `ResourceTemplate`, `Prompt` and `PromptArgument` hold `String`
(not `Cow`) in rmcp itself, so `resources/list`, `resources/templates/list` and
`prompts/list` still allocate their `uri`/`name`/`title`/`description`/
`mimeType` per request. That is rmcp's wire model, not R2E's: removing it means
changing rmcp. The R2E-side halves — build once, filter before cloning — are
both in place.

### The wire is unchanged

`r2e-mcp/tests/server/wire_golden.rs` pins the exact JSON of all four `*/list`
payloads (`r2e-mcp/tests/server/golden/*.json`, re-baselined with
`R2E_UPDATE_GOLDEN=1`). A representation change that alters the wire fails
there.

The goldens landed in the same commit as the change they guard, so they prove
nothing on their own — a golden captured *after* a regression records the
regression. Their provenance was established separately, and is reproducible:

```bash
git checkout -b tmp/golden-provenance c8da199   # master, pre-#994/#993
git show <pr-branch>:r2e-mcp/tests/server/wire_golden.rs \
  > r2e-mcp/tests/server/wire_golden.rs
echo 'mod wire_golden;' >> r2e-mcp/tests/server/main.rs
R2E_UPDATE_GOLDEN=1 cargo test -p r2e-mcp --test server wire_golden::
diff -r r2e-mcp/tests/server/golden <pr-checkout>/r2e-mcp/tests/server/golden
```

The test target compiles unchanged against master (it only uses
`#[mcp_routes]`'s public surface), so master can be made to emit its own
goldens. Run on **c8da199** (`Merge pull request #55`, the merge base of this
branch) all four files came out **byte-identical** to the ones committed here
— `tools_list`, `resources_list`, `resource_templates_list`, `prompts_list`.
`Cow::Borrowed` + `Tool::new_with_raw` is a representation change and nothing
more.

## One `AuthenticatedUser` per authenticated MCP request (task #993)

### The finding

The auth layer deposits the caller twice — standalone (what the identity
extractor reads) and inside the `McpPrincipal` (what scope checks read) — so
every authenticated request deep-copied the whole claims tree, flattened
`extra: serde_json::Map` included. The cost scaled with what the IdP put in
the token, on a path every MCP request takes.

### The fix

`McpPrincipal.user` is an `Arc<AuthenticatedUser>` (**breaking**: reads go
through `Deref`, `principal.user.sub` still works; an owned copy is now
`(*principal.user).clone()`). The identity is built once — in the JWT backend
and at both opaque-token construction sites — and shared from there on.

The layer then has to satisfy two readers with one allocation:

```rust
// r2e-mcp/src/auth/layer.rs
req.extensions_mut().insert(Arc::clone(&principal.user)); // refcount bump
req.extensions_mut().insert(principal);                   // holds the same Arc
```

The identity extension is therefore an `Arc<AuthenticatedUser>`, while
`#[inject(identity)] user: AuthenticatedUser` still hands the member an owned
value. `ToolCall::identity::<T>()` (also on `ResourceCall`/`PromptCall`, and
what the `#[tool]`/`#[resource]`/`#[prompt]` codegen now calls instead of
`extension::<T>()`) bridges the two: it looks for `Arc<T>` first and
materializes the owned `T` there — **once, and only for a member that actually
declares an identity parameter** — falling back to a plain `T` extension for
any other layer that inserts an identity by value. A member that declares
`Arc<AuthenticatedUser>` skips the copy entirely (that lookup hits the
fallback and finds the shared handle).

*Why not make the extension itself owned and share it the other way?* Because
the common member declares no identity at all: paying the copy in the layer
would charge every request for a value most of them never read. Paying it in
`identity::<T>()` charges only the members that ask.

### Before/after

`r2e-mcp/tests/hotpath/principal.rs` drives authenticated `tools/call`s
through the real layer against a validator that returns a prebuilt principal
(the shape of the opaque-token cache hit, where the identity *is* the whole
per-request cost), with a caller whose token carries 128 x 256-byte claims:

| | before | after |
|---|---|---|
| authenticated `tools/call`, no extra claims | 210 allocations / 34,692 bytes | **200 allocations / 32,976 bytes** |
| authenticated `tools/call`, ~32 KiB of claims | 764 allocations / 129,176 bytes | **200 allocations / 32,976 bytes** |
| `McpPrincipal::clone` (same claims) | 282 allocations / 47,300 bytes | **0 allocations** |

The per-request cost is now flat in the size of the claims tree. Exactly where
the refcounts move, since "three bumps" is only true of one of these paths:

| site | what it does | bumps |
|---|---|---|
| `layer.rs` identity deposit | `Arc::clone(&principal.user)` | 1 |
| `layer.rs` principal deposit | **moves** the principal into the extensions | 0 |
| `check_access` / `requirements_visible` | **borrow** `&McpPrincipal` out of the extensions | 0 |
| `McpPrincipal::clone` | derived: `user` + `scopes` (`token_hash` is a `u64`) | 2 |
| opaque cache hit (`TokenCache::get`) | clones the stored `Arc<McpPrincipal>` under the lock, then `(*principal).clone()` outside it | 3 |

So an authenticated JWT request costs one bump, and the opaque-cache-hit path
— the only place the number three appears — costs three: the outer cache `Arc`
plus the two inside the principal it materializes. No path copies claims.

## Global lifetimes and `Box::leak`

Where the fixes rely on a value living for the whole process, that lifetime is
either structurally guaranteed or contractually documented:

- **`&'static Metrics` (`r2e-prometheus/src/metrics.rs:7`)** — genuinely
  guaranteed: a `static METRICS: OnceLock<Metrics>` written at most once by
  `init_metrics`/`metrics()` (`get_or_init`) and only ever read by shared
  reference. `OnceLock` never drops or replaces its contents, so the `&'static`
  it hands out is sound by construction. This is what lets the per-request path
  hold no metric handles at all.
- **`&'static str` labels** — `r2e-http/src/labels.rs` and `name_for_algorithm`
  (`r2e-security/src/jwks.rs`) hand out string literals: genuinely `'static`,
  nothing allocated or leaked.
- **`status_label` (`r2e-prometheus/src/metrics.rs:134`) is *not* `&'static`** —
  it writes the status digits into a caller-owned `[u8; 5]` and returns a borrow
  of that stack buffer (`fn status_label(status: u16, buf: &mut [u8; 5]) ->
  &str`). Same outcome for the hot path — no allocation per request — by a
  different mechanism: the label lives on the caller's frame for the duration of
  the record call, so it can never outlive it.
- **`Arc<…>` everywhere else.** The audit's fixes deliberately use `Arc`
  (`Arc<MetricsConfig>`, `Arc<[String]>`, `Arc<str>`, `Bytes`) rather than
  leaking, so the data is freed with the app/plugin/decorator that owns it and
  a `#[cfg(feature = "dev-reload")]` hot-patch cycle does not leak a copy per
  reload.

`Box::leak` audit (`rg 'Box::leak|\.leak\(\)'` over the workspace, `vendor/`
excluded) — **five calls at three sites**, each with a lifecycle contract:

| Site | Calls | Leaks | Bounded by | Contract |
|---|---|---|---|---|
| `r2e-openfga/src/typed.rs:323` (`intern_wildcard::<T>`) | 1 | one `Box<str>` (`"<type>:*"`) per FGA type | the FGA types the process ever takes a wildcard of | **fallback only.** `model!` emits the wire form as the `FgaType::WILDCARD` literal, so a generated type never reaches this; a hand-written `FgaType` impl that leaves `WILDCARD` at `None` interns once per type, on first use. Documented in-place, guarded by `r2e-openfga/tests/typed.rs` |
| `r2e-devservices/src/service.rs:445` | 1 | one `OnceCell<DevService>` per (service, configuration) | the number of distinct dev-service specs in a test run | test harness only; the cell owns the shared container for the process's lifetime, which is exactly why it must be `&'static`. Documented in-place |
| `r2e-core/tests/plugin/deferred.rs:117-119` | 3 | three small test fixtures | one per test | test code, process-scoped fixture |

**On the request path:** only the OpenFGA one is reachable from production
request code — `FgaSubject::subject_str` is called by `FgaClient::{check,
grant,revoke}`, so before this workstream's follow-up a request using a
wildcard subject could be the one that performs the leak. It was never *per
request* (the interning cache is consulted first), and after the
`FgaType::WILDCARD` change a `model!`-generated model does not leak at all: the
wildcard subject is a compile-time literal. What remains is bounded by the
number of hand-written `FgaType` impls the process takes a wildcard of, one
small `Box<str>` each, on first use.

Nothing was added by this workstream: the OpenFGA and dev-services calls
predate it (the OpenFGA one was narrowed by it) and the rest are test code.

## Deferred

Each of these is a real per-request cost that was **not** fixed here, because the
fix is structural rather than "wrap it in an `Arc`". Listed so they are not
re-discovered from scratch.

### Shim metadata is re-rendered per request

`r2e-mcp/src/auth/shim.rs:184` deep-clones the IdP's whole metadata `Value`,
applies fixed rewrites and re-serializes it on every mirrored-metadata GET. The
result is identical for every caller between discovery refreshes, so it should
be memoized per discovery generation (keyed on the `Arc` pointer of the
`OAuthServerMetadata`). Low traffic, so it is a correctness-of-shape issue more
than a hot path; the three-copy render was reduced to one here.

### Rate-limit scope keys are formatted per request

`r2e-rate-limit/src/guard.rs:529,588` builds `"{controller}:{method}"` and then
a second `String` for the bucket key on every guarded request. The first cannot
simply be precomputed for the same reason as `Cache::full_key`: `#[guard]` on an
impl block yields one shared guard whose `method_name` varies per call site. A
small `OnceLock`-per-call-site cache inside the guard, or moving the scope into
the prebuilt `DecoratorSpec` per route, would fix it — both are more invasive
than this pass allowed.

### Generated head group extracts `HeaderMap`/`Extensions` by value

`r2e-macros/src/codegen/handlers.rs` (775/780, 1097/1102, 1482/1486,
1539/1543, 1624/1628, 1995/1999) extracts `HeaderMap` and `Extensions` by value
into the generated head group, but only on routes that need them (guards,
`#[managed]` params). These are per-request data rather than app-lifetime
config, so they are not in this audit's category; borrowing from `Parts`
instead would still be a measurable win on guarded routes.

### Stateless MCP allocates legacy session state per request

`r2e-mcp/src/plugin.rs:413-419` — in `mcp.stateless = true`, rmcp calls the
service factory per request, so `R2eMcpHandler::new` allocates an
`Arc<RwLock<HashSet<String>>>` and an `Arc<AtomicBool>` for legacy-session
tracking that stateless mode can never use. Two small allocations; make them
lazy.

## Not actionable

- `r2e-mcp/src/handler.rs:440` `self.rt.info.clone()` — rmcp's
  `ServerHandler::get_info` returns `ServerInfo` by value.
- tower-http's `Cors<S>` allocates a `Vec<HeaderValue>` for `Vary` on every
  service clone (`tower-http-0.6.11/src/cors/vary.rs:12`). Third-party, three
  cheap `HeaderValue`s.
- `r2e-security/src/identity.rs:380` — `claims.sub`/`claims.email` cloned into
  `AuthenticatedUser`; per-token data the struct must own.
