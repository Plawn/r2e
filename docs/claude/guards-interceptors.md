# Guards & Interceptors

Guards and interceptors are **graph-resolved decorators** (Phase 6): the
`#[guard(...)]` / `#[pre_guard(...)]` / `#[intercept(...)]` expression is
evaluated **once at controller registration**, its bean deps are declared at
the type level and checked at `register_controller()` (a missing bean is a
compile error naming the type, same UX as `#[inject]`), and the built values
are captured by the handler closure — one `Arc` per route. Per-request cost:
one `Arc` clone + monomorphized calls. There is **no state access at request
time** (no `BeanLookup`, no per-request construction).

## The DecoratorSpec contract (`r2e-core/src/decorator.rs`)

The attribute expression's **leading type path** names the spec, and the
expression must evaluate to it:

| Attribute expression | Spec type |
|---|---|
| `#[guard(MyGuard)]` | `MyGuard` |
| `#[guard(MyGuard("key"))]` | `MyGuard` (single-segment uppercase call = tuple-struct ctor) |
| `#[roles("admin")]` → `RolesGuard { .. }` | `RolesGuard` |
| `#[guard(RateLimit::per_user(5, 60))]` | `RateLimit` |
| `#[intercept(Cache::ttl(30).group("x"))]` | `Cache` (builder chains return `Self`) |
| `#[intercept(DbAudit::spec("api"))]` | `DbAudit` (`#[derive(DecoratorBean)]` constructor) |
| `#[guard(MyGuard = make_guard())]` | `MyGuard` (escape hatch for free fns/vars) |

`#[routes]` emits `build_decorator::<_, Spec>(expr, ctx)` per site inside
`Controller::routes(state, core, ctx)`, and folds `<Spec as
DecoratorSpec>::Deps` into `Controller::Deps`. `build_decorator`
(`r2e-core/src/decorator.rs`) bounds the expression's own spec type to the
named one with `Product`/`Deps` equality — for hand-written specs the two
coincide; `#[derive(DecoratorBean)]` splits them (see below) and the bounds
keep the dep fold exact.

```rust
pub trait DecoratorSpec: Sized {
    type Product: Send + Sync + 'static;  // the guard/interceptor built
    type Deps;                            // TCons list of beans build() pulls
    fn build(self, ctx: &BeanContext) -> Self::Product;
}
```

Three ways to satisfy it:

- **Self-contained** (no bean deps — the expression already is the finished
  decorator): one line, `impl SelfBuilt for MyGuard {}` (blanket impl gives
  `Product = Self, Deps = TNil`). The blanket coexists with downstream
  config-type impls (negative coherence; a type must not be both).
- **Bean-reading — `#[derive(DecoratorBean)]`** (the normal route): one
  struct, fields split by attribute; the derive generates the spec plumbing:

```rust
#[derive(DecoratorBean)]
pub struct DbAudit {
    #[inject] pool: PgPool,          // from the bean graph (compile-checked)
    #[config("audit.channel")] channel: String, // from R2eConfig
    tag: &'static str,               // plain = config, set at the site
}

impl<R: Send> Interceptor<R> for DbAudit { /* uses self.pool */ }

// at the site — plain fields in declaration order:
#[intercept(DbAudit::spec("api"))]
```

  Generated: a hidden companion spec `__R2eSpec_DbAudit` (plain fields)
  returned by `DbAudit::spec(...)`, its real `DecoratorSpec` impl, and an
  identity `DecoratorSpec` impl on `DbAudit` itself carrying the same `Deps`
  (what the controller fold reads, and what makes the
  `#[guard(DbAudit = prebuilt)]` escape hatch work). Not supported: enums,
  tuple structs, generics.
- **Bean-reading — manual spec** (low-level; what the derive expands to):
  the expression evaluates to a pure config value whose impl names the
  product and pulls beans in `build`:

```rust
pub struct DbAudit;                       // spec (named by the attribute)
pub struct DbAuditReady { pool: PgPool }  // product (holds the bean)

impl DecoratorSpec for DbAudit {
    type Product = DbAuditReady;
    type Deps = TCons<PgPool, TNil>;
    fn build(self, ctx: &BeanContext) -> DbAuditReady {
        DbAuditReady { pool: ctx.get() }
    }
}
```

## Guards

Handler-level guards run before the handler body and can short-circuit with
an error response. The `Guard<I: Identity>` trait
(`r2e-core/src/guards.rs`) defines async
`check(&self, ctx) -> Result<(), Response>` — **no state parameter**; a
guard's beans are fields, injected at build time.

`GuardContext<'a, I: Identity>` provides:
- `method_name`, `controller_name` — handler identification
- `headers` — request headers (`&HeaderMap`)
- `uri` — request URI (`&Uri`) with convenience methods `path()` and `query_string()`
- `path_params` — typed path parameters (`path_param()`, `parse_path_param()`)
- `identity` — optional identity reference (`Option<&'a I>`)
- Convenience accessors: `identity_sub()`, `identity_email()`, `identity_claims()`

The `Identity` trait (`r2e-core::Identity`) decouples guards from the
concrete `AuthenticatedUser` type: `sub()` (required), `email()` /
`claims()` (optional). Role access lives on `RoleBasedIdentity` in
`r2e-security`. `NoIdentity` is the sentinel when no identity is available.

### Built-in guards

- `RolesGuard` / `AllRolesGuard` — role checks, 403 on failure. Applied via
  `#[roles("admin")]` / `#[all_roles(...)]` (desugared to a `RolesGuard`
  struct-literal guard site). Self-built.
- `RateLimitGuard` — token-bucket rate limiting, 429. The spec is the
  `RateLimit` config value; the guard holds the `RateLimitRegistry` bean:
  ```rust
  use r2e::r2e_rate_limit::{PreRateLimit, RateLimit};

  #[pre_guard(PreRateLimit::global(5, 60))]   // 5 req / 60 sec, shared bucket (pre-auth)
  #[pre_guard(PreRateLimit::per_ip(5, 60))]   // 5 req / 60 sec, per IP (pre-auth)
  #[guard(RateLimit::per_user(5, 60))]        // 5 req / 60 sec, per user (post-auth)
  ```
  The app must `.provide(RateLimitRegistry::default())` — checked at compile
  time for app-level controllers.
- `FgaGuard` (r2e-openfga) — the spec is the `FgaCheck` builder value; the
  guard holds the `OpenFgaRegistry` bean.

### Pre-authentication guards

For checks that don't need identity (IP rate limiting, allowlisting):
`PreAuthGuard` (no generics). Pre-auth guards run as middleware **before**
JWT extraction. Context: `PreAuthGuardContext` (no identity). They are
prebuilt like everything else (`__R2ePreDeco_*` set, one `Arc` captured by
the middleware closure). SSE and WS endpoints support `#[pre_guard]` too.

### Custom guards

- Post-auth: implement `Guard<I: Identity>` (async via RPITIT), apply with
  `#[guard(MyGuard)]`.
- Pre-auth: implement `PreAuthGuard`, apply with `#[pre_guard(MyPreGuard)]`.
- No bean deps → add `impl SelfBuilt for MyGuard {}`. Tuple-struct config
  works directly: `#[guard(RequireApiKey("x-api-key"))]`.
- Bean deps → `#[derive(DecoratorBean)]` with `#[inject]` fields, applied
  with `#[guard(MyGuard::spec(...))]`; hand-write `DecoratorSpec` on a
  config type only when the derive doesn't fit (see the contract above).
  Never look beans up at request time.

## Interceptors

Cross-cutting concerns (logging, timing, caching) implement `Interceptor<R>`
with an `around` pattern (`r2e-core/src/interceptors.rs`). All calls are
monomorphized (no `dyn`). `InterceptorContext` is a `Copy` struct
`{ method_name, controller_name }` — no state field.

### Built-in interceptors (in `r2e-utils`)

- `Logged` — logs entry/exit at a configurable `LogLevel`. Self-built.
- `Timed` — measures execution time, optional threshold. Self-built.
- `Counted` / `MetricTimed` — named counter / duration metric via `tracing`. Self-built.
- `Cache` — caches `Cacheable` responses. **Spec**: the product holds the
  `Arc<dyn CacheStore>` bean — the app must provide one
  (`.provide(InMemoryStore::shared())`); a missing store is a compile error.
  There is no global store anymore (`cache_backend()` was deleted).
- `CacheInvalidate` — clears a named cache group after the method. Same
  store bean.

## Execution order (outermost → innermost)

Pre-auth middleware level (runs BEFORE Axum extraction/JWT validation):
0. `pre_guard(PreRateLimit::global/per_ip(...))`, custom pre-auth guards

Handler level (after extraction, before controller body):
1. Guards in declaration order — `#[roles]`/`#[all_roles]` desugar to guard
   sites that run first, then `#[guard(...)]` sites top-to-bottom
2. Validation (garde)

Method body level (trait-based, via `Interceptor::around`):
3. Controller-level interceptors (declaration order)
4. Method-level interceptors (declaration order)

Instance lifetime: every site (including controller-level ones, which are
instantiated **once per route method**) is built at registration and lives
for the app's lifetime — a stateful interceptor keeps its state across
requests, and controller-level state is per-method, not shared.

Inline codegen (no trait):
5. `transactional` (wraps body in tx begin/commit)
6. Original method body

**Design invariant:** Interceptors always see the handler's **raw return
type** (`Json<T>`, `Result<Json<T>, E>`, etc.), never `Response`. The
`IntoResponse::into_response()` conversion happens *after* the outermost
interceptor. Guards short-circuit *before* interceptors.

## Cache interceptor type constraints

`Cache` requires `R: Cacheable`. Built-in `Cacheable` impls:
- `Json<T>` where `T: Serialize + DeserializeOwned + Send`
- `Result<T, E>` where `T: Cacheable, E: Send` (only caches `Ok` values)
- Types deriving `#[derive(Cacheable)]`

Other built-in interceptors only require `R: Send` and work with any return type.

```rust
#[intercept(Counted::new("user_list_total"))]           // count invocations
#[intercept(MetricTimed::new("user_list_duration"))]    // record duration as named metric
async fn list(&self) -> Json<Vec<User>> { /* ... */ }
```

## Combining interceptors with guards/roles

`#[intercept(Cache)]` + `#[roles]` (or any `#[guard]`) works correctly —
guards run first, then interceptors see the raw type:
```rust
#[get("/admin/users")]
#[roles("admin")]
#[intercept(Cache::ttl(30).group("admin_users"))]
async fn admin_list(&self) -> Json<Vec<User>> { /* ... */ }
```

**Known limitations:**
- `#[managed]` + `#[intercept(Cache)]` does NOT work — the managed resource
  lifecycle wraps `into_response` inside the interceptor closure, so `Cache`
  sees `Response` instead of the raw type. Workaround: read the store bean
  (`#[inject] store: Arc<dyn CacheStore>`) and cache manually in the body.
- **Scheduled and gRPC method interceptors are graph-built too** (since
  di-next-steps item 5). Scheduled sets are built once inside
  `scheduled_tasks_boxed(state, core, ctx)` and stored in the core's hidden
  `DecoSlot` field (added by `#[controller]` to every core); gRPC sites are
  prebuilt into the hidden `__R2eGrpc<Name>` wrapper at `into_router`.
  Bean-reading specs work in both places. Scheduled spec deps are folded
  into `ControllerDeps` and compile-checked like route decorator deps; gRPC
  deps (core AND decorators) are NOT compile-checked —
  `register_grpc_service` resolves from the retained context at runtime, so
  a missing bean panics there (pre-existing gRPC behavior, unchanged).
- **Scheduled methods intercept DIRECT calls too** (user decision): the
  chain runs in the method's dispatch wrapper (slot lookup), so
  `self.tick().await` from another method goes through the interceptors —
  unlike routes, whose interceptors live in the handler. A **sync**
  `#[scheduled]` method with `#[intercept]` sites gets its wrapper
  **promoted to `async fn`** (di-next-steps item 11): the source `fn` body
  stays sync (hidden inner fn), but callers must `.await` the generated
  method — the promotion is flagged in the generated rustdoc. One edge: a
  core that never went through registration (hand-built `from_context` in a
  test) has an empty slot → direct calls run undecorated.
- **Module controllers' decorator deps ARE compile-checked** (since the
  post-Phase-6 `ControllerDeps` carrier): they register through the
  unchecked backend, but the module-scope check folds the full
  `ControllerDeps::Deps` list (core ++ decorator deps), so a guard or
  interceptor reading a bean outside `Provides ∪ Imports` is a compile
  error at `register_module` — declare the bean in the module's `imports`.

## Configurable syntax

```rust
#[transactional]                             // uses self.pool
#[transactional(pool = "read_db")]           // custom pool field
#[pre_guard(PreRateLimit::global(5, 60))]    // global rate limit (pre-auth)
#[pre_guard(PreRateLimit::per_ip(5, 60))]    // per-IP rate limit (pre-auth)
#[guard(RateLimit::per_user(5, 60))]         // per-user rate limit (post-auth, requires identity)
#[guard(MyCustomGuard)]                      // custom post-auth guard (SelfBuilt or spec)
#[guard(RequireApiKey("x-api-key"))]         // SelfBuilt tuple-struct guard with config
#[guard(MyBeanGuard::spec(5))]               // #[derive(DecoratorBean)] guard (plain fields as args)
#[guard(MyGuard = make_guard())]             // escape hatch: explicit spec type
#[pre_guard(MyPreAuthGuard)]                 // custom pre-auth guard (runs before JWT)
#[intercept(MyInterceptor)]                  // user-defined decorator
#[intercept(Logged::info())]                 // built-in interceptor with config
#[intercept(Cache::ttl(30).group("users"))]  // cache with named group (needs the store bean)
#[intercept(CacheInvalidate::group("users"))] // invalidate cache group
#[intercept(Counted::new("metric_name"))]    // count invocations
#[intercept(MetricTimed::new("metric_name"))] // record duration as named metric
#[middleware(my_middleware_fn)]               // Tower middleware via from_fn
#[layer(TimeoutLayer::new(Duration::from_secs(5)))] // arbitrary Tower Layer
#[status(200)]                               // override OpenAPI status code
#[returns(MyType)]                           // explicit OpenAPI response type
#[raw]                                       // marker for raw Axum extractors (no-op)
```

Guard expressions can reference typed path-parameter descriptors — the
per-method `path` module is scoped to the decorator constructor:
```rust
#[guard(ProjectGuard::viewer(path::id))]
async fn show(&self, Path(id): Path<ProjectId>) { ... }
```

## Tower Middleware & Layers

### `#[middleware]` — Tower middleware functions

Applies a Tower middleware function to a specific route via
`r2e::http::middleware::from_fn`. Signature: `async fn(Request, Next) -> Response`.

```rust
#[get("/data")]
#[middleware(require_api_key)]
async fn protected_data(&self) -> Json<Vec<Item>> { /* ... */ }
```

Multiple `#[middleware]` attributes stack, outermost-first in declaration
order. Generated code calls `.layer(r2e::http::middleware::from_fn(name))`.

### `#[layer]` — Arbitrary Tower layers

Accepts any expression evaluating to a Tower `Layer` (e.g. `tower-http`
layers). Can be combined with `#[middleware]` on the same route.

Common layers: `TimeoutLayer`, `SetResponseHeaderLayer`, `CorsLayer`,
`CompressionLayer`, `ConcurrencyLimitLayer`.

## Route Annotation Attributes

### `#[status(CODE)]` — Override HTTP status code for OpenAPI

Overrides the default success status code in the generated OpenAPI spec (not
the actual HTTP response). Defaults: GET→200, POST→201, PUT→200, PATCH→200,
DELETE→204.

### `#[returns(Type)]` — Explicit response type for OpenAPI

Declares the response body type when the macro cannot infer it
(`impl IntoResponse`, custom wrappers). Combines with `#[status]`.

### `#[raw]` — Mark raw Axum extractors

Documentation-only marker with no effect on code generation; signals a raw
Axum extractor parameter alongside `#[inject(identity)]` / `#[managed]`
params.
