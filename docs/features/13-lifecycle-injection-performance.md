# Feature 13 — Lifecycle, Dependency Injection, and Performance Implications

## Overview

This document describes the complete lifecycle of an R2E application — from startup to shutdown — as well as the internals of dependency injection and its performance implications.

---

## 1. Application Lifecycle

### 1.1 Assembly Phase (`AppBuilder`)

Everything starts with the fluent construction via `AppBuilder`:

```rust
AppBuilder::new()
    .with_config(config)            // 1. Configuration
    .with_state(services)           // 2. Etat applicatif
    .with_cors()                    // 3. Layers Tower
    .with_tracing()
    .with_health()
    .with_error_handling()
    .with_openapi(openapi_config)   // 4. Documentation
    .with_scheduler(|s| {           // 5. Taches planifiees
        s.register::<ScheduledJobs>();
    })
    .on_start(|state| async { Ok(()) })  // 6. Hooks
    .on_stop(|_| async { })
    .register_controller::<UserController>()  // 7. Controllers
    .serve("0.0.0.0:3000")         // 8. Lancement
    .await?;
```

`AppBuilder` accumulates elements without executing anything. Assembly happens when `build()` or `serve()` is called.

### 1.2 Internal Construction (`build_inner`)

The `build_inner()` method produces a tuple `(Router, StartupHooks, ShutdownHooks, ConsumerRegs, State)`:

1. **Axum Router creation** — an empty `Router<T>`
2. **Route merging** — each controller registers its routes via `Controller::routes()`
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
    .with_state(services)
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
    +-- __R2eExtract_<Name>  ← construction du controller
    |       +-- #[inject(identity)] : FromRequestParts (async)
    |       +-- #[inject]           : state.field.clone() (sync)
    |       +-- #[config("key")]    : config.get(key) (sync)
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

### 2.2 Controller Extraction

The generated extractor `__R2eExtract_<Name>` implements `FromRequestParts<State>`. It constructs the controller in three phases:

**Phase 1 — Identity (async, fallible)**

```rust
let user = <AuthenticatedUser as FromRequestParts<State>>
    ::from_request_parts(parts, state)
    .await
    .map_err(IntoResponse::into_response)?;
```

This is the only asynchronous phase. For `AuthenticatedUser`, this involves:
- Extracting the `Authorization: Bearer <token>` header
- JWT validation (cryptographic signature verification)
- JWKS lookup if the key is not cached (potentially a network call)
- Constructing the `AuthenticatedUser` object

If extraction fails, the request is immediately rejected (401).

**Phase 2 — Inject (sync, infallible)**

```rust
user_service: state.user_service.clone(),
pool: state.pool.clone(),
```

Each `#[inject]` field is cloned from the state. Purely synchronous operation.

**Phase 3 — Config (sync, panics if missing)**

```rust
greeting: {
    let cfg = <R2eConfig as FromRef<State>>::from_ref(state);
    cfg.get("app.greeting").unwrap_or_else(|e| panic!(...))
}
```

Extraction of `R2eConfig` from the state via `FromRef`, then a `HashMap` lookup.

### 2.3 Two Handler Modes

**Simple mode** (without guards) — the handler directly returns the method's return type:

```rust
async fn __r2e_UserController_list(
    ctrl_ext: __R2eExtract_UserController,
    // ... params
) -> Json<Vec<User>> {
    let ctrl = ctrl_ext.0;
    ctrl.list().await
}
```

**Guarded mode** (with `#[roles]`, `#[rate_limited]`, `#[guard]`) — the handler returns `Response` to allow short-circuiting:

```rust
async fn __r2e_UserController_admin_list(
    State(state): State<Services>,
    headers: HeaderMap,
    ctrl_ext: __R2eExtract_UserController,
) -> Response {
    let guard_ctx = GuardContext {
        method_name: "admin_list",
        controller_name: "UserController",
        headers: &headers,
        identity: guard_identity(&ctrl_ext.0),  // Option<&AuthenticatedUser>
    };

    // Short-circuit si le guard echoue
    if let Err(resp) = Guard::check(&RolesGuard { required_roles: &["admin"] }, &state, &guard_ctx) {
        return resp;
    }

    let ctrl = ctrl_ext.0;
    IntoResponse::into_response(ctrl.admin_list().await)
}
```

**Implications**: in guarded mode, Axum also extracts `State` and `HeaderMap` in addition to the controller extractor. State extraction is an additional clone (but cheap — it is an internal `Arc` clone).

---

## 3. Dependency Injection: The Three Scopes

### 3.1 `#[inject]` — Application Scope

| Property | Value |
|----------|-------|
| Resolution | Compile-time (codegen) |
| Timing | On each request |
| Operation | `state.field.clone()` |
| Prerequisite | `Clone + Send + Sync` |
| Fallible | No |
| Async | No |

**Generated code:**
```rust
field_name: __state.field_name.clone()
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
| Operation | `FromRequestParts::from_request_parts()` |
| Prerequisite | `FromRequestParts<State>` + `Identity` |
| Fallible | Yes (error response) |
| Async | Yes |

**Generated code:**
```rust
let user = <AuthenticatedUser as FromRequestParts<State>>
    ::from_request_parts(__parts, __state)
    .await
    .map_err(IntoResponse::into_response)?;
```

**Two possible locations:**

- **On the struct** — the controller always requires the identity. No `StatefulConstruct`.
- **On a handler parameter** — only annotated handlers require the identity. `StatefulConstruct` is generated.

**Cost**: this is the most expensive scope. For `AuthenticatedUser`, each request involves JWT validation with cryptographic signature verification.

### 3.3 `#[config("key")]` — Application Scope (lookup)

| Property | Value |
|----------|-------|
| Resolution | Compile-time (codegen) |
| Timing | On each request |
| Operation | `FromRef` + `HashMap::get()` |
| Prerequisite | `FromConfigValue` |
| Fallible | Panics if key is missing |
| Async | No |

**Generated code:**
```rust
field_name: {
    let __cfg = <R2eConfig as FromRef<State>>::from_ref(__state);
    __cfg.get("app.greeting").unwrap_or_else(|e| panic!(...))
}
```

**Note**: the config is cloned from the state (via `FromRef`), then a HashMap lookup is performed. If the key does not exist, the handler **panics** (and `CatchPanicLayer` converts it to a 500).

### 3.4 Summary Diagram

```
                    ┌─────────────────────────────────────────────┐
                    │           Etat applicatif (State)            │
                    │                                             │
                    │  user_service: UserService  ←── Arc interne │
                    │  pool: SqlitePool           ←── Arc interne │
                    │  jwt_validator: Arc<JwtValidator>            │
                    │  event_bus: LocalEventBus     ←── Arc interne │
                    │  config: R2eConfig       ←── HashMap     │
                    │  rate_limiter: RateLimitRegistry             │
                    └──────────────┬──────────────────────────────┘
                                   │
                    ┌──────────────┴──────────────────────────────┐
                    │         Requete HTTP entrante                │
                    └──────────────┬──────────────────────────────┘
                                   │
            ┌──────────────────────┼──────────────────────────┐
            │                      │                          │
    #[inject]              #[inject(identity)]         #[config("key")]
    state.field.clone()    FromRequestParts(async)     config.get(key)
    ↓                      ↓                           ↓
    O(1) si Arc            Validation JWT              O(1) HashMap
    Sync, infaillible      Async, faillible (401)      Sync, panic si absent
```

---

## 4. Construction Outside HTTP Context: `StatefulConstruct`

The `StatefulConstruct<S>` trait allows constructing a controller from the state alone, without an HTTP request. It is automatically generated by `#[derive(Controller)]` **only** when the struct has no `#[inject(identity)]` field.

### 4.1 Usage by Consumers

```rust
// Code genere par #[routes] pour #[consumer(bus = "event_bus")]
event_bus.subscribe(move |event: Arc<UserCreatedEvent>| {
    let state = state.clone();
    async move {
        let ctrl = <MyController as StatefulConstruct<State>>::from_state(&state);
        ctrl.on_user_created(event).await;
    }
}).await;
```

### 4.2 Usage by Scheduled Tasks

```rust
// Code genere par #[routes] pour #[scheduled(every = 30)]
scheduler.add_task(ScheduledTask {
    name: "MyController_cleanup",
    schedule: Schedule::Every(Duration::from_secs(30)),
    task: Box::new(move |state: State| {
        Box::pin(async move {
            let ctrl = <MyController as StatefulConstruct<State>>::from_state(&state);
            ctrl.cleanup().await;
        })
    }),
});
```

### 4.3 The Mixed Controller Pattern

With `#[inject(identity)]` on handler **parameters** (not on the struct), the controller retains `StatefulConstruct` while still allowing protected endpoints:

```rust
#[derive(Controller)]
#[controller(path = "/api", state = Services)]
pub struct MixedController {
    #[inject] user_service: UserService,
    // Pas de #[inject(identity)] ici → StatefulConstruct genere
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
    async fn cleanup(&self) { ... }  // Fonctionne car StatefulConstruct existe
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
    pub identity: Option<&'a I>,
}

pub trait Guard<S, I: Identity>: Send + Sync {
    fn check(&self, state: &S, ctx: &GuardContext<'_, I>) -> Result<(), Response>;
}
```

The `Identity` trait decouples guards from the concrete `AuthenticatedUser` type. Built-in guards (`RolesGuard`, `RateLimitGuard`) are generic over `I: Identity`.

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
// Appel a la fonction du meta-module
let guard_ctx = GuardContext {
    identity: __r2e_meta_Name::guard_identity(&ctrl_ext.0),
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
| **`#[inject]` field cloning** | Sync | **~10-50 ns per field** | With `Arc` types (atomic refcount) |
| **Config lookup** | Sync | **~50 ns per field** | HashMap lookup + type conversion |
| **JWT validation** | Async | **~10-50 us** | Cryptographic signature verification |
| **JWKS lookup (cache miss)** | Async | **~50-200 ms** | HTTP call to the OIDC provider |
| Rate limit guard | Sync | ~100 ns | Token bucket check |
| Roles guard | Sync | ~50 ns | Iteration over the roles array |
| Interceptors | Async | ~100 ns overhead | Monomorphized, zero vtable |
| Business logic | Async | Variable | Database I/O, external services |

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
| `StatefulConstruct` | Not generated | Generated |
| Consumers / Schedulers | Impossible | Possible |
| Identity access in self | `self.user` (always available) | Not available in self |
| Guard context | Via `guard_identity(&ctrl)` | Via reference to param |
| Public endpoint overhead | Unnecessary JWT validation | No JWT overhead |

**Recommendation**: use the param-level pattern for controllers that mix public and protected endpoints. Reserve struct-level for fully protected controllers where the identity is used in most methods.

### 6.6 Scheduled Tasks: Construction Cost

Each scheduled task execution calls `StatefulConstruct::from_state`, which clones `#[inject]` fields and looks up `#[config]` fields. For high-frequency tasks (e.g., `every = 1`), this cost is identical to that of an HTTP request (minus identity extraction).

**Recommendation**: for very high-frequency tasks, reduce the number of injected fields to the minimum necessary.

---

## 7. Summary of Golden Rules

1. **Wrap services in `Arc<T>`** — per-request cloning becomes a simple atomic increment
2. **Prefer param-level `#[inject(identity)]`** for mixed controllers — avoids JWT validation on public endpoints
3. **Limit the number of `#[config]` fields** — each field clones the entire `R2eConfig`
4. **Pre-warm the JWKS cache** at startup if first-request latency matters
5. **Interceptors are free** in terms of dispatch overhead — the cost is in their internal logic
6. **Guards must remain synchronous and O(1)** — no I/O in a guard
7. **One controller per responsibility** — avoids injecting unnecessary dependencies that are cloned on every request
