# Feature 13 — Lifecycle, Dependency Injection, and Performance Implications

## Overview

This document describes the complete lifecycle of an R2E application — from startup to shutdown — as well as the internals of dependency injection and its performance implications.

---

## 1. Application Lifecycle

### 1.1 Assembly Phase (`AppBuilder`)

Everything starts with the fluent construction via `AppBuilder`:

```rust
AppBuilder::new()
    .load_config::<RootConfig>()          // 1. Configuration (typee)
    .plugin(Executor)                     // 2. Plugins pre-build (pool requis par Scheduler)
    .plugin(Scheduler)                    // 2. Plugins pre-build (taches planifiees)
    .provide(pool)                        // 3. Beans pre-construits (par type)
    .provide(event_bus)
    .register::<UserService>()            // 4. Beans #[bean] / #[producer]
    .build_state()                        // 5. Resolution du graphe → etat HList infere
    .await
    .with(Health)                         // 6. Plugins HTTP
    .with(Cors::permissive())
    .with(ErrorHandling)
    .on_start(|_state| async move { Ok(()) })  // 7. Hooks
    .on_stop(|_| async {})
    .register_controllers::<(UserController, ScheduledJobs)>()  // 8. Controllers
    .serve("0.0.0.0:3000")                // 9. Lancement
    .await?;
```

`AppBuilder` accumulates registrations without executing anything. `build_state()`
(no type arguments, `async`) resolves the bean graph and materializes the provision
list into an **HList state** — the state type is *inferred* from the `.provide` /
`.register` calls; you never write a state struct. Controllers are registered
**after** `build_state()`, and final assembly happens when `serve()` is called.

> **Note**: apps with more than ~127 registered beans need
> `#![recursion_limit = "512"]` in each crate root (`main.rs` and `lib.rs`). `r2e doctor` warns
> as the bean count approaches the threshold.

### 1.2 Internal Construction (`build_inner`)

The `build_inner()` method produces a tuple `(Router, StartupHooks, ShutdownHooks, ConsumerRegs, State)`:

1. **Axum Router creation** — an empty `Router<T>`
2. **Route merging** — each controller receives its shared core via `Controller::routes(&state, core)`
3. **OpenAPI** (if enabled) — invocation of the OpenAPI builder with collected metadata, adding `/openapi.json` and `/docs` routes
4. **System routes** — `/health` and `/__r2e_dev/*` if enabled
5. **State application** — `router.with_state(state.clone())`: a single clone at construction time
6. **Layer stacking** — applied in reverse declaration order (the last added is the outermost)

### 1.3 Tower Layer Order

Layers stack from inside to outside. At runtime, the request traverses them in reverse order:

```
Requete HTTP entrante
        |
        v
 [TraceLayer]          -- log de la requete/reponse (le plus externe)
 [CatchPanicLayer]     -- capture les panics → JSON 500
 [CorsLayer]           -- validation CORS (le plus proche du handler)
        |
        v
   Handler Axum
```

**Implication**: `TraceLayer` sees all requests, including those rejected by CORS. Panics in the handler are caught by `CatchPanicLayer` and converted into a clean JSON 500 response.

### 1.4 Startup Sequence (`serve`)

```
serve(addr)
    |
    |-- 1. build_inner() → Router + Hooks + ConsumerRegs + State
    |
    |-- 2. Enregistrement des consumers d'evenements
    |       Pour chaque controller avec #[consumer] :
    |         → Controller::register_consumers(state.clone())
    |         → Subscribe les handlers sur le bus d'evenements
    |
    |-- 3. Execution des hooks on_start (dans l'ordre d'enregistrement)
    |       Chaque hook recoit state.clone()
    |       Un hook qui echoue arrete le demarrage
    |
    |-- 4. Binding TCP sur l'adresse
    |
    |-- 5. axum::serve() avec graceful shutdown
    |       Le serveur accepte des connexions
    |       En arriere-plan : taches planifiees actives
    |
    |-- 6. Signal d'arret (Ctrl-C / SIGTERM)
    |       → Arret des nouvelles connexions
    |       → Attente de la fin des requetes en cours
    |
    |-- 7. Execution des hooks on_stop (dans l'ordre d'enregistrement)
    |
    └-- 8. Arret
```

### 1.5 Graceful Shutdown

The scheduler uses a `CancellationToken` (from `tokio-util`) that is cancelled in an `on_stop` hook registered by `with_scheduler`. Each scheduled task watches this token via `tokio::select!` and stops cleanly.

In-flight HTTP requests are completed before closing (default Axum behavior).

### 1.6 Shutdown Grace Period

By default, the process waits indefinitely for shutdown hooks to complete. `shutdown_grace_period(Duration)` sets a maximum delay:

```rust
AppBuilder::new()
    .build_state()
    .await
    .shutdown_grace_period(Duration::from_secs(5))
    .serve("0.0.0.0:3000").await?;
```

If the hooks (plugin + user) do not finish within the delay, the process forces shutdown via `process::exit(1)`. This guarantees that a blocking hook does not leave the process hanging indefinitely.

---

## 2. HTTP Request Lifecycle

### 2.1 Overview

```
Requete HTTP
    |
    v
[Layers Tower]  ← TraceLayer → CatchPanicLayer → CorsLayer
    |
    v
[Routage Axum]  ← correspondance path + method
    |
    v
[Extraction]    ← pipeline d'extracteurs Axum
    |
    +-- State (si handler guarde)
    +-- HeaderMap (si handler guarde)
    +-- Arc<Core>            ← clone de l'Arc du core (construit une fois a l'enregistrement)
    +-- __R2eRequestData_<Name>  ← FromRequestParts : valeurs request-scoped uniquement
    |       +-- #[inject(identity)] : FromRequestParts (async)
    |       +-- #[inject(request)]  : FromRequestParts (async)
    |   (les #[inject] / #[config] vivent sur le core, deja construits)
    +-- bind_request → façade __R2eRequest_<Name> (Deref vers le core)
    +-- Params handler (Json, Path, Query, etc.)
    +-- #[inject(identity)] param (si param-level)
    |
    v
[Guards]        ← execution sequentielle, short-circuit sur erreur
    |
    +-- RateLimitGuard → 429 Too Many Requests
    +-- RolesGuard     → 403 Forbidden
    +-- Custom Guards  → reponse custom
    |
    v
[Intercepteurs] ← chain around() monomorphisee
    |
    +-- Logged (entering)
    +-- Timed (start)
    +-- User interceptors
    +-- Cache (lookup)
    |       +-- Hit  → retour immediat
    |       +-- Miss → continue
    |
    v
[Corps du handler]
    |
    +-- transactional (begin tx)
    +-- logique metier
    +-- transactional (commit/rollback)
    |
    v
[Post-traitement]
    |
    +-- Cache (store si miss)
    +-- CacheInvalidate (clear group)
    +-- Timed (log elapsed)
    +-- Logged (exiting)
    |
    v
Reponse HTTP
```

### 2.2 Controller Core and Request Façade

`#[controller]` splits every controller into two physical pieces:

- a **core** struct holding only application-scoped data (`#[inject]` and `#[config]`
  fields). The core is built **once** at registration time via
  `ContextConstruct::from_context(&BeanContext)` — each field resolved from the bean
  graph **by type** — and shared as an `Arc<Core>`. The `#[inject]`/`#[config]`
  resolution described below therefore happens **once at registration**, not per request.
- a generated **request façade** (`__R2eRequest_<Name>`) holding the request-scoped fields
  plus an `Arc` to the core, with `Deref<Target = Core>`. Inside a route body, `self.user`
  is a direct façade field while `self.service` resolves to the core through autoderef.

Each registered route closure captures the core `Arc` and, per request, runs two steps:

**Step 1 — Core construction (once, at registration)**

```rust
// Built once when the controller is registered, shared across all requests.
let core: Arc<UserController> = Arc::new(UserController::from_context(&ctx));
// Inside from_context (ctx: &BeanContext) — resolution by TYPE from the graph:
//   user_service: ctx.get::<UserService>(),           // #[inject], sync
//   greeting: ctx.get::<R2eConfig>()                   // #[config], sync
//       .get("app.greeting")
//       .unwrap_or_else(|e| panic!(...))
```

**Step 2 — Request data extraction (per request, async, fallible)**

The generated `__R2eRequestData_<Name>` extractor implements `FromRequestParts<S>`
(generic over the inferred HList state) and produces **only** the request-scoped values
(`#[inject(identity)]` and `#[inject(request)]`). When the controller declares no
request-scoped fields it is zero-sized and infallible.

Each value is resolved through the R2E-owned trait `FromRequestPartsVia<S, M>` (or
`OptionalFromRequestPartsVia<S, M>` for `Option<T>` fields). The marker slot `M`
carries the `HasBean` witness: for `AuthenticatedUser`, it locates the
`Arc<JwtClaimsValidator>` bean inside the state with a monomorphized fixed-offset
access. Plain axum `FromRequestParts<S>` extractors still work unchanged, bridged
automatically through the blanket `ViaAxum` marker.

```rust
let user = <AuthenticatedUser as FromRequestPartsVia<S, M>>
    ::from_request_parts_via(parts, state)
    .await
    .map_err(IntoResponse::into_response)?;
```

For `AuthenticatedUser`, this involves:
- Extracting the `Authorization: Bearer <token>` header
- JWT validation (cryptographic signature verification)
- JWKS lookup if the key is not cached (potentially a network call)
- Constructing the `AuthenticatedUser` object

If extraction fails, the request is immediately rejected (401). The extracted values are
then moved, together with the core `Arc`, into the façade via
`__r2e_meta_<Name>::bind_request`. There is no per-request DI re-resolution: `#[inject]` and
`#[config]` are read from the shared core, never recomputed.

### 2.3 Two Handler Modes

Every endpoint shares the same shape: the Axum-facing closure captures the core `Arc`,
extracts `__R2eRequestData_<Name>`, binds the stack façade with `bind_request`, then runs
the route method on the façade.

**Simple mode** (without guards) — the closure directly returns the method's return type:

```rust
// core: Arc<UserController> est capture une fois a l'enregistrement.
let core = core.clone();
move |data: __R2eRequestData_UserController, /* ... params */| {
    let core = core.clone(); // un clone d'Arc par requete
    async move {
        let ctrl = __r2e_meta_UserController::bind_request(core, data);
        ctrl.list(/* params */).await
    }
}
```

**Guarded mode** (with `#[roles]`, `#[rate_limited]`, `#[guard]`) — the closure also extracts
`State` and `HeaderMap` and returns `Response` to allow short-circuiting:

```rust
let core = core.clone();
let deco = deco.clone();   // guards/interceptors built once from the BeanContext at registration
move |headers: HeaderMap,
      uri: Uri,
      data: __R2eRequestData_UserController| {
    let core = core.clone();
    let deco = deco.clone();
    async move {
        let ctrl = __r2e_meta_UserController::bind_request(core, data);

        let guard_ctx = GuardContext {
            method_name: "admin_list",
            controller_name: "UserController",
            headers: &headers,
            uri: &uri,
            path_params: PathParams::EMPTY,
            identity: __r2e_meta_UserController::guard_identity(&ctrl), // Option<&AuthenticatedUser>, lu sur la façade
        };

        // Guard built at registration (deco.g0); check() takes no state, and is async.
        if let Err(resp) = Guard::check(&deco.g0, &guard_ctx).await {
            return resp;
        }

        IntoResponse::into_response(ctrl.admin_list().await)
    }
}
```

**Implications**: in guarded mode, Axum extracts `HeaderMap` and `Uri` in addition to the request-data extractor, to build the `GuardContext`. There is **no `State` extraction** — the guard was constructed once at registration and is captured by the closure. In all modes the per-request cost is one `Arc` clone of the core, one clone of the prebuilt decorator bundle, and one request-data extraction; the core and the guards are built once at registration.

---

## 3. Dependency Injection: The Three Scopes

### 3.1 `#[inject]` — Application Scope

| Property | Value |
|----------|-------|
| Resolution | Compile-time (codegen; missing bean = compile error naming the type) |
| Timing | Once at registration (into the shared core `Arc`) |
| Operation | `ctx.get::<T>()` — by type, from the resolved `BeanContext` |
| Prerequisite | `Clone + Send + Sync`, present in the bean graph (`.provide` / `.register`) |
| Fallible | No |
| Async | No |

**Generated code:**
```rust
field_name: ctx.get::<FieldType>()
```

**Common patterns:**

| Type | Clone cost | Mechanism |
|------|-----------|-----------|
| `Arc<T>` | O(1) — atomic refcount increment | Immutable sharing |
| `SqlxPool` | O(1) — internal `Arc` | Connection pool |
| `LocalEventBus` | O(1) — `Arc<RwLock<HashMap>>` | Event bus |
| `RateLimitRegistry` | O(1) — internal `Arc` | Rate limiter registry |
| `R2eConfig` | O(n) — `HashMap` clone | Configuration |

**Best practice**: wrap heavy services in `Arc<T>` so that cloning is a simple atomic reference increment. The framework does not require `Arc`, but the provided types (`SqlxPool`, `LocalEventBus`, etc.) already use it internally.

### 3.2 `#[inject(identity)]` — Request Scope

| Property | Value |
|----------|-------|
| Resolution | Compile-time (codegen) |
| Timing | On each request |
| Operation | `FromRequestPartsVia::from_request_parts_via()` |
| Prerequisite | `FromRequestPartsVia<S, M>` + `Identity` (plain axum `FromRequestParts<S>` extractors are bridged automatically via `ViaAxum`) |
| Fallible | Yes (error response) |
| Async | Yes |

**Generated code:**
```rust
let user = <AuthenticatedUser as FromRequestPartsVia<S, M>>
    ::from_request_parts_via(__parts, __state)
    .await
    .map_err(IntoResponse::into_response)?;
```

For `AuthenticatedUser`, the marker `M` resolves a `HasBean<Arc<JwtClaimsValidator>, _>`
witness — the validator must be provided as a bean
(`.provide(Arc::new(JwtClaimsValidator::...))`) and is read from the HList state with a
fixed-offset access (no lookup).

**Two possible locations:**

- **On the struct** — every HTTP route on the controller authenticates. The identity lives
  on the per-request façade, never on the core, so `ContextConstruct` is still generated for
  the core (the dependencies are not rebuilt per request).
- **On a handler parameter** — only annotated handlers require the identity. `ContextConstruct`
  is generated for the core as well.

In both cases the core is built once and `ContextConstruct` is always generated; only the
small façade (one `Arc` clone plus the extracted identity) is created per request.

**Cost**: this is the most expensive scope. For `AuthenticatedUser`, each request involves JWT validation with cryptographic signature verification.

### 3.3 `#[config("key")]` — Application Scope (lookup)

| Property | Value |
|----------|-------|
| Resolution | Compile-time (codegen) |
| Timing | Once at registration (into the shared core `Arc`) |
| Operation | `ctx.get::<R2eConfig>()` + `HashMap::get()` |
| Prerequisite | `FromConfigValue`; `R2eConfig` is itself a bean in the graph |
| Fallible | Fails at startup if key is missing |
| Async | No |

**Generated code:**
```rust
field_name: {
    let __cfg = ctx.get::<R2eConfig>();
    __cfg.get("app.greeting").unwrap_or_else(|e| panic!(...))
}
```

**Note**: the config is pulled from the bean graph by type (`R2eConfig` is a bean), then a
HashMap lookup by string key is performed — **once, at core construction**. If the key does
not exist, registration fails **at startup** with a readable error (`register_controller`
panics; `try_register_controller` returns a `Result` instead) — never mid-request.

### 3.4 Summary Diagram

```
                    ┌─────────────────────────────────────────────┐
                    │   Etat applicatif = HList infere (forme=P)   │
                    │   (aucune struct ecrite par l'utilisateur)   │
                    │                                             │
                    │  [ UserService ]            ←── Arc interne │
                    │  [ SqlitePool ]             ←── Arc interne │
                    │  [ Arc<JwtClaimsValidator> ]                 │
                    │  [ LocalEventBus ]          ←── Arc interne │
                    │  [ R2eConfig ]              ←── HashMap     │
                    │  [ RateLimitRegistry ]                       │
                    └──────────────┬──────────────────────────────┘
                                   │
                    ┌──────────────┴──────────────────────────────┐
                    │         Requete HTTP entrante                │
                    └──────────────┬──────────────────────────────┘
                                   │
            ┌──────────────────────┼──────────────────────────┐
            │                      │                          │
    #[inject]              #[inject(identity)]         #[config("key")]
    ctx.get::<T>()         FromRequestPartsVia         ctx.get::<R2eConfig>()
    (1x, enregistrement)   (async, par requete)        .get(key) (1x, enreg.)
    ↓                      ↓                           ↓
    O(1) si Arc            Validation JWT              O(1) HashMap
    Sync, infaillible      Async, faillible (401)      Sync, echec au demarrage
```

---

## 4. Construction Outside HTTP Context: `ContextConstruct`

The `ContextConstruct` trait constructs a controller **core** from the resolved `BeanContext` alone, without an HTTP request — each `#[inject]` field resolved from the graph **by type** via `ctx.get::<T>()`, each `#[config]` field read from the `R2eConfig` bean. It is **always** generated by `#[controller]`: identity and `#[inject(request)]` fields live only on the per-request façade, never on the core, so the core can always be built from the context. The builder retains the graph as an `Arc<BeanContext>` through the typed phase; `register_controller()` calls `from_context` **once** and wires routes, consumers, and scheduled tasks to that **same shared core** (these run on the core and cannot access request identity).

### 4.1 Usage by Consumers

Consumers capture the shared core `Arc` at registration; each event delivery is one `Arc` clone:

```rust
// Code genere par #[routes] pour #[consumer(bus = "event_bus")]
// __core: Arc<Self> — le core partage, construit une fois via from_context
let __consumer_core = __core.clone();
event_bus.subscribe(move |__envelope: EventEnvelope<UserCreatedEvent>| {
    let __ctrl = __consumer_core.clone();  // un clone d'Arc par evenement
    async move {
        __ctrl.on_user_created(__envelope.event).await.into()
    }
}).await;
```

### 4.2 Usage by Scheduled Tasks

Scheduled tasks likewise capture the shared core; each execution is one `Arc` clone.
The generated `ScheduledTaskDef` no longer clones the app state on every tick — it
carries `state: ()`, and the tick body is submitted to the shared `PoolExecutor`:

```rust
// Code genere par #[routes] pour #[scheduled(every = 30)]
let __task_core = __core.clone();
ScheduledTaskDef {
    name: "MyController_cleanup".to_string(),
    schedule: Schedule::Every(Duration::from_secs(30)),
    state: (),                             // plus de clone d'etat par tick
    task: Box::new(move |_state| {
        let __ctrl = __task_core.clone();  // un clone d'Arc par execution
        Box::pin(async move {
            ScheduledResult::log_if_err(__ctrl.cleanup().await, "MyController_cleanup");
        })
    }),
}
```

The scheduler runtime submits each tick to the `PoolExecutor` and awaits its
`JobHandle` before the next tick — so ticks are drained on shutdown, bounded by
`executor.max-concurrent`, and a panicking tick is contained without killing its
schedule loop.

### 4.3 The Mixed Controller Pattern

With `#[inject(identity)]` on handler **parameters** (not on the struct), the controller keeps
request scope explicit per endpoint while still allowing protected endpoints. (Struct-level
identity also keeps `ContextConstruct` on the core — it simply makes every HTTP route
authenticate.)

```rust
#[controller(path = "/api")]
pub struct MixedController {
    #[inject] user_service: UserService,
    // Identity au niveau parametre → request scope explicite par endpoint
}

#[routes]
impl MixedController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Vec<Data>> { ... }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<AuthenticatedUser> {
        Json(user)
    }

    #[scheduled(every = 60)]
    async fn cleanup(&self) { ... }  // Fonctionne car ContextConstruct existe
}
```

---

## 5. Guards and the `Identity` Trait

### 5.1 Architecture

```rust
// r2e-core
pub trait Identity: Send + Sync {
    fn sub(&self) -> &str;
    fn roles(&self) -> &[String];
}

pub struct GuardContext<'a, I: Identity> {
    pub method_name: &'static str,
    pub controller_name: &'static str,
    pub headers: &'a HeaderMap,
    pub uri: &'a Uri,
    pub path_params: PathParams<'a>,
    pub identity: Option<&'a I>,
}

pub trait Guard<I: Identity>: Send + Sync {
    fn check(&self, ctx: &GuardContext<'_, I>)
        -> impl Future<Output = Result<(), Response>> + Send;
}
```

The `Identity` trait decouples guards from the concrete `AuthenticatedUser` type. Built-in guards (`RolesGuard`, `RateLimitGuard`) are generic over `I: Identity`.

Guards are **graph-resolved decorators** (Phase 6): they are built **once, at controller
registration**, from the resolved `BeanContext` — never per request, and `check` takes
**no state parameter**. A guard that reads no beans is self-contained (`impl SelfBuilt for
MyGuard {}`); a guard that needs a bean holds it as a field, and a spec type named by the
`#[guard(...)]` expression implements `DecoratorSpec` (Product + Deps + build) to pull the
bean from the graph. A missing bean is a compile error at `register_controller()`.

### 5.2 Identity Source for Guards

Two cases in generated code:

**Case A** — Identity on a handler parameter:

```rust
// Le param est deja extrait par Axum
let guard_ctx = GuardContext {
    identity: Some(&__arg_0),  // reference directe au param
    ...
};
```

**Case B** — Identity on the struct (or absent):

```rust
// Appel a la fonction du meta-module sur la façade (ctrl)
let guard_ctx = GuardContext {
    identity: __r2e_meta_Name::guard_identity(&ctrl),
    ...
};
```

When there is no identity at all, `guard_identity` returns `None` and the type is `NoIdentity`. A guard like `RolesGuard` then returns 403 "No identity available for role check".

---

## 6. Performance Implications

### 6.1 Per-Request Cost — Breakdown

| Step | Type | Typical Cost | Notes |
|------|------|-------------|-------|
| Tower layers | Sync | ~1 us | Tracing, CORS, error handling |
| Axum routing | Sync | ~1 us | Radix tree matching |
| **Core `Arc` clone** | Sync | **~10 ns** | One per request; `#[inject]`/`#[config]` already resolved into the core at registration |
| **App-dep access in request extractors** | Sync | **~ns (fixed-offset)** | `HasBean` on the HList state — monomorphized field access, no hash/lookup |
| **JWT validation** | Async | **~10-50 us** | Cryptographic signature verification |
| **JWKS lookup (cache miss)** | Async | **~50-200 ms** | HTTP call to the OIDC provider |
| Rate limit guard | Sync | ~100 ns | Token bucket check |
| Roles guard | Sync | ~50 ns | Iteration over the roles array |
| Interceptors | Async | ~100 ns overhead | Monomorphized, zero vtable |
| Business logic | Async | Variable | Database I/O, external services |

`#[inject]` field resolution (`ctx.get::<T>()`, ~10-50 ns per field with `Arc` types) and
`#[config]` lookups (~50 ns per field) happen **once at registration**, not per request.

### 6.2 Critical Operations in Detail

#### State Cloning (`#[inject]`)

Cloning happens on every request for each `#[inject]` field. This is the Axum mechanism: the `FromRequestParts` extractor receives an immutable reference to the state and must produce a local copy.

**Recommendation**: use `Arc<T>` for expensive-to-clone services. The framework already does this for `SqlxPool`, `LocalEventBus`, and `RateLimitRegistry`.

```rust
// Bon : Arc<T> → clone O(1)
#[derive(Clone)]
pub struct Services {
    pub user_service: UserService,       // contient Arc<RwLock<Vec<User>>>
    pub jwt_validator: Arc<JwtValidator>,
    pub pool: SqlitePool,                // Arc interne
}

// Mauvais : si UserService contenait Vec<User> directement → clone O(n) par requete
```

**Anti-pattern**: storing `R2eConfig` as an `#[inject]` field instead of via `#[config]`. `R2eConfig` is a `HashMap<String, ConfigValue>` — its clone copies the entire map on every request. Prefer `#[config("key")]` which only clones the requested value, or store the config as `Arc<R2eConfig>`.

#### JWT Validation (`#[inject(identity)]`)

This is generally the most expensive extraction operation. It includes:

1. **Header parsing** — O(1), negligible
2. **JWT decoding** — base64 + JSON parsing, ~1 us
3. **Signature verification** — RSA/ECDSA, ~10-50 us depending on algorithm
4. **Key lookup** (JWKS mode):
   - Cache hit: RwLock read, ~100 ns
   - Cache miss: HTTP request to the JWKS endpoint, ~50-200 ms
5. **`AuthenticatedUser` construction** — allocation, negligible

**Possible optimizations**:

- **Pre-warm the JWKS cache** in an `on_start` hook (first request without latency)
- **Static key in dev** (`JwtValidator::new_with_static_key`) avoids JWKS entirely
- **The JWKS cache is shared** via `Arc<RwLock>` — a single refresh even under load

**Struct-level vs param-level**: when the identity is on the struct, it is extracted for **all** requests to that controller, even endpoints that do not need it. The param-level pattern (`#[inject(identity)]` on the param) avoids this extraction for public endpoints.

#### Configuration Lookup (`#[config]`)

Each `#[config("key")]` field performs:

1. `FromRef` extraction of `R2eConfig` — clone of the `HashMap`, O(n) where n = number of keys
2. `config.get(key)` — O(1) lookup + type conversion

**The config clone is the point of concern**. If the config contains 100 keys, that is 100 allocations per `#[config]` field per request.

**Recommendation**: for high-throughput controllers, prefer injecting config values into the state at startup rather than via `#[config]`:

```rust
// Plutot que :
#[config("app.greeting")] greeting: String,

// Considerer :
#[inject] greeting: Arc<String>,  // pre-construit dans l'etat
```

### 6.3 Interceptors: Zero-Cost Abstraction

Interceptors use the `Interceptor<R>` trait which is **monomorphized** by the Rust compiler:

```rust
pub trait Interceptor<R> {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F)
        -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send;
}
```

- **No `dyn` dispatch** — the concrete interceptor type is known at compile time
- **Nested closure inlining** — LLVM optimizes nested closures into linear code
- **`InterceptorContext` is `Copy`** — captured by value in each async closure

The real cost of interceptors is that of their **business logic** (logging, timing, cache lookup), not the `around` mechanism.

### 6.4 Guards: Synchronous Execution

Guards are executed synchronously within an async handler. They do not block the Tokio runtime because they are typically O(1):

- `RolesGuard` — iteration over a small roles slice
- `RateLimitGuard` — access to a `DashMap` (lock-free for reads)

**Warning**: a custom guard that performs I/O would block the runtime. Guards must remain fast and synchronous.

### 6.5 Struct-Level vs Param-Level Identity Comparison

| Aspect | Struct-level `#[inject(identity)]` | Param-level `#[inject(identity)]` |
|--------|-----------------------------------|----------------------------------|
| JWT extraction | On every request, all endpoints | Only annotated endpoints |
| `ContextConstruct` | Generated (on the core) | Generated (on the core) |
| Consumers / Schedulers | Possible (run on the core) | Possible (run on the core) |
| Identity access in self | `self.user` (façade field, always available) | Not available in self |
| Guard context | Via `guard_identity(&ctrl)` | Via reference to param |
| Public endpoint overhead | Unnecessary JWT validation | No JWT overhead |
| Per-request DI cost | One core `Arc` clone (deps built once) | One core `Arc` clone (deps built once) |

**Recommendation**: use the param-level pattern for controllers that mix public and protected endpoints. Reserve struct-level for fully protected controllers where the identity is used in most methods. Note that struct-level identity no longer rebuilds the controller's dependencies per request — the shared core is built once; only the façade carries the per-request identity.

### 6.6 Scheduled Tasks: Construction Cost

Scheduled tasks share the controller core built once at `register_controller()`
(from the resolved bean graph via `ContextConstruct`): each execution clones the
core `Arc` — the same cost model as HTTP requests. No per-run dependency
resolution or config lookup occurs.

---

## 7. Summary of Golden Rules

1. **Wrap services in `Arc<T>`** — per-request cloning becomes a simple atomic increment
2. **Prefer param-level `#[inject(identity)]`** for mixed controllers — avoids JWT validation on public endpoints
3. **Limit the number of `#[config]` fields** — each field clones the entire `R2eConfig`
4. **Pre-warm the JWKS cache** at startup if first-request latency matters
5. **Interceptors are free** in terms of dispatch overhead — the cost is in their internal logic
6. **Guards must remain synchronous and O(1)** — no I/O in a guard
7. **One controller per responsibility** — avoids injecting unnecessary dependencies that are cloned on every request
