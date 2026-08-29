# Hot-path clone audit — per-request deep clones of immutable data

Date: **2026-08-29** — task #990 acceptance criterion "framework middleware /
per-request paths that implicitly deep-clone immutable configuration or other
immutable structures per request".

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
   (`r2e-core/src/builder/typed.rs:849`, `router.with_state(state.clone())`).

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
| `r2e-mcp` | `src/auth/layer.rs:167` | `principal.user.clone()` — the same `AuthenticatedUser` inserted twice so both extensions exist | yes | deferred — see below |
| `r2e-mcp` | `src/handler.rs:121,182` | `Vec<Tool>` / `Vec<Resource>` / `Vec<Prompt>` / `Vec<ResourceTemplate>` cloned per `*/list` | yes | deferred — see below |
| `r2e-mcp` | `src/auth/layer.rs` `AuthState`, `auth/validator.rs`, `auth/discovery.rs` | `Arc` / `Arc<str>` / `Arc<[String]>` only; scope checks borrow `&StandardClaims` | yes, but `Arc` | clean |
| `r2e-mcp` | `src/uri_template.rs:243-296` | template variable names copied into the per-call `BTreeMap<String, String>` | yes, per `resources/read` | acceptable — forced by the public `ResourceCall::variables` type |
| `r2e-grpc` | `src/multiplex.rs` `call()` | `self.grpc.clone()` / `self.http.clone()` | yes | clean — `tonic::service::Routes` wraps `axum::Router`, whose `Clone` is an `Arc` bump (verified in `tonic-0.14.6/src/service/router.rs:14`, `axum-0.8.9/src/routing/mod.rs:72`) |
| `r2e-grpc` | `src/guard.rs`, `src/identity.rs` | — | yes | clean — `GrpcGuardContext` borrows the metadata map, bearer extraction returns `&str`, roles are `&'static [&'static str]` |
| `r2e-tenant` | `src/{extract,id,router,resolver,plugin,map/*}.rs` | `TenantId(Arc<str>)`, `TenantRouter = Arc<Mode>`, `Tenanted<T>` = `Arc` inner, `TenantedSettings: Copy`, resolvers hold `Cow<'static, str>` and borrow it | yes, but `Arc`/`Copy` | clean — the whole crate is already built to this invariant |
| `r2e-tenant` | `src/source.rs:195` | `ResolutionChain::root::<T>()` allocates a 1-element `Vec` on every `Tenanted::get()`, including cache hits, though it is only read when a `create` runs | yes | acceptable — one small `Vec`, not a clone of shared data; could be made lazy

## Deferred

Each of these is a real per-request cost that was **not** fixed here, because the
fix is structural rather than "wrap it in an `Arc`". Listed so they are not
re-discovered from scratch.

### The bean HList is cloned into every request

`r2e-core/src/builder/typed.rs:849` installs the resolved bean HList as the
router state, and `axum/src/handler/service.rs:168` clones that state on every
request whether or not the handler declares `State<S>`. `HCons`
(`r2e-core/src/type_list.rs:147-153`) derives `Clone`, so the cost is the sum of
every bean's `Clone`. In practice beans are `Arc`-shaped by convention, so this
is usually N refcount bumps rather than N deep copies — but nothing *enforces*
it, and N grows with the app.

The fix is to hold the HList behind a single `Arc` so the per-request clone is
one bump regardless of N. That touches `BeanAccess`, `HasBean`,
`BeanLookup`, `FromRequestPartsVia` and the generated extractor in
`r2e-macros`, which is well beyond a minimal change. Worth its own task, with a
benchmark on a wide graph first.

### `McpPrincipal` carries `AuthenticatedUser` by value

`r2e-mcp/src/auth/layer.rs:167` inserts the same `AuthenticatedUser` twice —
once standalone (for the identity extractor) and once inside `McpPrincipal` —
so every authenticated MCP request deep-copies the claims tree, including the
flattened `extra: serde_json::Map`. Making `McpPrincipal.user` an
`Arc<AuthenticatedUser>` removes it, but is a breaking change for anything
reading `principal.user` by value. Acceptable pre-production; out of scope for
this pass.

### MCP `*/list` clones the whole wire list

`r2e-mcp/src/handler.rs:121,182` clone the prebuilt `Vec<Tool>` /
`Vec<Resource>` / `Vec<Prompt>` / `Vec<ResourceTemplate>` on every list request
(not once per session — clients re-list, and `list_changed` forces it). rmcp's
`Tool` uses `Cow<'static, str>` for `name`/`description`, but `ToolRoute` stores
them as `Option<String>` (`r2e-mcp/src/route.rs:118-120`), so `to_rmcp_tool`
produces `Cow::Owned` and every clone re-allocates each string — even though the
macro emits them from string literals. Storing `Cow<'static, str>` in
`ToolRoute` would make the tool list clone allocation-free; `Resource`/`Prompt`
hold `String` in rmcp itself, so those need the visibility filter to be checked
before rebuilding rather than after.

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
