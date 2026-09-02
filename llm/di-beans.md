---
topic: di-beans
features: core
tokens: ~3700
requires: core-concepts
---

## Dependency Injection — beans

### TL;DR

- `#[bean]` on an impl for types you own (sync/async auto-detected), `#[producer]`
  on a function for types you don't (pools, clients); the producer emits a struct
  named after the function (`create_pool` → `CreatePool`), and both are installed
  with the one unified `.register::<T>()`.
- Return `Result<T, E>` to fail construction, and write the **literal**
  `Result<T, E>`: a one-argument alias (`anyhow::Result<T>`) is rejected. Never
  `panic!` / `process::exit` in a constructor — return the error.
- An `unsafe fn` constructor is rejected: drop `unsafe` from the signature and
  keep the `unsafe { }` block, with its SAFETY comment, in the body.
- Constructor parameters are resolved from the graph **by type**;
  `#[config("key")]` parameters/fields read config, and `Option<T>` there makes
  the key optional instead of a startup error.
- Async constructors are deliberately **not** `Send`-bound — hold a `!Send` value
  across `.await` and do not reach for a `spawn_blocking` + `block_on` workaround.
  Inside a `#[bean(lazy)]` constructor use `rt::spawn_blocking`, never
  `rt::block_in_place`.
- `build_state()` takes **no type arguments** — the state is inferred; past ~127
  provisions add `#![recursion_limit = "512"]` to every crate root including the app.
- Use `#[producer(after(A, B))]` for a dependency the body never reads; naming a
  type that is already a parameter there is a compile error.
- `#[derive(ProvideBundle)]` + `.provide_all(env)` provides one bean per field in
  field order; an `R2eConfig` field is applied as `.override_config(..)`, so call
  `provide_all` **before** `load_config`.
- `Option<T>` is a first-class bean type: the slot always exists, so dependents
  always compile and the producer decides `Some`/`None`.
- There are no qualifiers or named beans — two beans of the same underlying type
  need two **newtypes**.

### Three bean kinds, one `register()`

| Trait | Constructor | Use case |
|-------|-------------|----------|
| `Bean` | sync `fn new(...) -> Self` / `-> Result<Self, E>` | Simple services |
| `AsyncBean` | `async fn new(...) -> Self` / `-> Result<Self, E>` | Async init |
| `Producer` | `#[producer] async fn ...() -> T` / `-> Result<T, E>` | Types you don't own (pools, clients) |

```rust
#[derive(Clone)]
pub struct UserService {
    pool: SqlitePool,
    event_bus: LocalEventBus,
}

#[bean]                                   // sync/async auto-detected
impl UserService {
    pub fn new(pool: SqlitePool, event_bus: LocalEventBus) -> Self {
        Self { pool, event_bus }
    }
}

#[producer]                               // for types you don't own
async fn create_pool(#[config("database.url")] url: String) -> Result<SqlitePool, sqlx::Error> {
    Ok(SqlitePool::connect(&url).await?)   // no expect/panic: the failure is reported
}
// Generates `struct CreatePool` — register with `.register::<CreatePool>()`
```

**Attributes on a `#[producer]` function are preserved** — doc comments,
`#[allow]`/`#[deny]`, `#[inline]`, `#[deprecated]`, `#[must_use]` all reach the
re-emitted function. The macro additionally emits
`#[allow(clippy::too_many_arguments)]` on every producer (one parameter per
dependency makes the 7-argument threshold meaningless there) and a doc comment on
the generated struct (so `#![deny(missing_docs)]` crates keep building); write
`#[warn(clippy::too_many_arguments)]` on the function to opt back in.

Attributes are likewise preserved on the `impl` block of `#[routes]`, and
associated `const`s/`type`s written in a `#[routes]` block stay on the controller
core (a route body's `Self` is the request façade, so reach them through the
controller name: `MyController::PAGE_SIZE`). `#[routes]` replaces the written
`impl` with **two** synthesized ones (routes on the request façade, everything
else on the core) and copies the impl attributes to both, so only **inert**
attributes may sit below `#[routes]`: doc comments,
`#[allow]`/`#[warn]`/`#[deny]`/`#[expect]`/`#[forbid]`, `#[deprecated]`,
`#[cfg]`, `#[cfg_attr]`, and tool attributes (`#[rustfmt::skip]`). An attribute
macro there is a compile error (it would expand twice) — put it **above**
`#[routes]`, where it runs exactly once.

`#[cfg]` (or `#[cfg_attr]`) on a **request-scoped** controller field
(`#[inject(identity)]` / `#[inject(request)]`) is a compile error — cfg the whole
controller instead. Other attributes on a request-scoped field are projected onto
the generated extractor and façade, so a `#[deprecated]` request field warns
where *you* read it, not from inside generated code.

An **`unsafe fn`** `#[producer]` — or a `#[bean]` constructor — is rejected. R2E
generates a safe `Producer::produce` / `Bean::build` that is the graph's only
caller, and it cannot discharge an `unsafe` contract; drop `unsafe` from the
signature and keep the `unsafe { }` block, with its SAFETY comment, in the body.

**Ordering-only dependencies.** `#[producer(after(A, B))]` adds `A` and `B` to
the producer's `Deps` / `dependencies()` — so the graph builds them first and a
missing one is the usual boot error — **without** binding a parameter. Use it
for a bean the body never reads (a process-wide guard, a migration runner, a
registry that must already be populated), instead of the unused-parameter idiom
it replaces:

```rust
#[producer(after(InstanceGuard, Migrations))]
fn create_db(#[config("database.url")] url: String) -> Db { Db::open(&url) }
```

Naming a type that is already a parameter is a compile error (a parameter is
already an edge). `after(..)` composes with `#[producer(start)]`.

**Fallible construction.** Any of the three may return `Result<_, E>` for any
`E: Into<BootError>` (`BootError = Box<dyn Error + Send + Sync>`). The error type
is an associated `type Error` on the trait — it never contaminates the bean type:
the producer above registers `SqlitePool`, and consumers inject `SqlitePool`, not
`Result<SqlitePool, _>`. An infallible constructor (`-> Self` / `-> T`) gets
`type Error = Infallible` from the macro; a hand-written `impl Bean` /
`impl AsyncBean` / `impl Producer` must declare `type Error` itself.

The split is **textual**: only a literal `Result<T, E>` return type is read as
fallible. A one-argument alias (`anyhow::Result<T>`, `std::io::Result<T>`) hides
the error type from the macro, so `#[producer]` rejects it with a message asking
for `Result<T, anyhow::Error>` (or `-> T`) rather than registering the bean
under the alias itself with `Error = Infallible`.

The first failure aborts the build: `try_build_state()` returns
`BeanError::BeanBuild { bean, source }` — `"Bean '<type>' failed to build: <e>"`,
with the original error kept as `source()` — and the beans already constructed in
that cycle are dropped as the stack unwinds. `build_state()` is the panicking
form of the same thing. So a constructor **returns** its error instead of calling
`process::exit`/`panic!`: under `TestApp::boot` the failure becomes one failing
test naming the cause, and under `app_main!` one `error:` line and exit code 1.

`#[config("key")]` works in `#[bean]` constructor params and `#[derive(Bean)]`
fields. Constructor params are resolved from the bean graph **by type**.

A `#[config("key")] x: Option<T>` param/field is **optional**: a missing key
resolves to `None` (not a startup error), an explicit `null` also yields `None`,
and a present value yields `Some(v)` (a type mismatch still fails loudly). This
applies everywhere `#[config]` is accepted — `#[producer]`/`#[bean]` params,
`#[derive(Bean)]`/`#[derive(DecoratorBean)]`/`#[derive(BackgroundService)]`
fields, `#[controller]` fields, and `#[derive(ConfigProperties)]` fields (where
an explicit `null` also falls back to `#[config(env = …)]` / `#[config(default
= …)]`, exactly like an absent key).

### Async constructors are NOT `Send`-bound

`AsyncBean::build` and `Producer::produce` return `impl Future<Output = _> + '_`
— deliberately **without** `+ Send`. The bean graph is resolved in place on the
boot thread (`build_state()` / `TestApp::boot` / `r2e::launch` await it, nothing
spawns it), so a constructor may hold a `!Send` value across an `.await`:

```rust
#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    pub async fn new(pool: PgPool) -> Self {
        let mut tx = pool.begin().await.unwrap();
        run_migration_step(&mut tx).await;   // generic over sqlx `Acquire` — fine
        tx.commit().await.unwrap();
        Self
    }
}
```

This used to be `+ Send`. Because an auto-trait bound on an RPITIT is checked
for *all* lifetimes, it rejected ordinary sqlx bodies — anything reborrowing a
transaction into an `Acquire`/`Executor`-generic helper — with:

```text
error: lifetime bound not satisfied
  --> src/beans.rs:18:1
   |
18 | #[producer]
   | ^^^^^^^^^^^
   |
   = note: this is a known limitation that will be removed in the future
           (see issue #100013 <https://github.com/rust-lang/rust/issues/100013>)
```

older rustc rendered the same thing as `implementation of `Executor` is not
general enough … `Executor<'1>` would have to be implemented for the type
`&'0 mut PgConnection`, for any two lifetimes `'0` and `'1``. If you see either
message, you do **not** need a `spawn_blocking` + `block_on` workaround — write
the body normally. `Plugin::build` has the same (unbounded) shape.

The one place a constructor future still crosses threads is a **lazy** bean
(`#[bean(lazy)]`) resolved from a sharded worker: its *factory closure* is
`Send + Sync` and runs on a dedicated `r2e-lazy-bean` thread that enters the
control-plane runtime, so the future itself stays `!Send`-friendly — but
`rt::block_in_place` inside a lazy constructor panics there; use
`rt::spawn_blocking`.

### Assembly — state is inferred

```rust
# async fn __doc() -> Result<(), Box<dyn std::error::Error>> {
let built = AppBuilder::new()
    .load_config::<RootConfig>()          // YAML + env; auto-registers config sections as beans
    .plugin(Executor)                     // required by Scheduler (ticks run on the pool)
    .plugin(Scheduler)                    // order vs load_config/provide/register doesn't matter
    .provide(LocalEventBus::new())        // provide constructed values
    .provide(Arc::new(claims_validator))
    .register::<CreatePool>()             // producer → registers SqlitePool
    .register::<UserService>()            // bean (sync or async — same call)
    .build_state()                        // async, NO type arguments — state is inferred
    .await;                               // .try_build_state().await = non-panicking
# Ok(()) }
```

**`build_state()` takes no type arguments.** The state is the provision list
materialized as a type-level HList, wrapped in `BeanState<L>` (one `Arc`)
before it reaches the router. Bean access is unchanged, each at the cost it
always had: `state.get::<T>()` still compiles to a fixed-offset field access
(now behind one pointer dereference), and `state.bean::<T>()` is still the
witness-free runtime `TypeId` walk down the list. `>127` provisions need
`#![recursion_limit = "512"]` in every crate root that includes the app
(`src/main.rs` and `src/lib.rs` in the standard layout).

### `#[derive(ProvideBundle)]` — provide a whole `App::Env` in one call

`App::setup` builds the process-lifetime resources; `App::build` then has to
hand every one of them to the graph, which is one `.provide(env.field)` line per
field. The derive turns the env struct itself into the provision list:

```rust
#[derive(ProvideBundle)]              // in the prelude
pub struct AppEnv {
    pub pool: SqlitePool,
    pub bus: LocalEventBus,
    pub s3: Option<S3Client>,         // provided as-is: Option<T> is a bean type
    pub config: R2eConfig,            // special: acts as .override_config(..)
}

async fn build(b: AppBuilder, env: AppEnv) -> Result<impl BootableApp, BootError> {
    Ok(b.provide_all(env)             // == .provide(env.pool).provide(env.bus)...
        .load_config::<()>()
        .try_build_state().await?)
}
```

- One provision per field, **in field order**, exactly as the hand-written
  `.provide(..)` chain would produce; the provision list `P` grows per field, so
  a missing bean is still a compile error at the registration that needs it.
  Every field must be `Clone + Send + Sync + 'static`.
- `Option<T>` fields are provided **as-is** — R2E treats `Option<T>` as its own
  bean type (see below), so there is no unwrapping and no "skip if `None`": a
  compile-time provision list cannot depend on a runtime value.
- An `R2eConfig` field is **not** provided as a bean: it is applied as
  `.override_config(value)`, which is why `provide_all` belongs before
  `load_config`. At most one such field (two is a compile error). Detection is
  **textual** on the written type — a field written `R2eConfig` is treated as the
  config even if it is an unrelated type of that name, and a type *alias* for
  `R2eConfig` is not recognised. Write the type as `R2eConfig`.
- `provide_all` is a pre-`build_state` builder method. Generic structs, tuple
  structs, unit structs and enums are rejected with a spanned error.

### `Option<T>` beans — conditional availability

`Option<T>` is a first-class bean type; the producer decides `Some`/`None`, the
slot always exists (so dependents always compile):

```rust
#[producer]
async fn create_llm(#[config("app.llm.api_key")] key: Option<String>) -> Option<Arc<LlmClient>> {
    Some(Arc::new(LlmClient::new(&key?)))
}

#[derive(Clone)]
pub struct ChatService { llm: Option<Arc<LlmClient>> }

#[bean]
impl ChatService {
    fn new(llm: Option<Arc<LlmClient>>) -> Self { Self { llm } }  // hard dep on the Option slot
}
```

`.plugin()` changes the compile-time provision list, so it can NOT be applied
behind a runtime flag: `.when()` only accepts `Self -> Self` (raw layers, config
toggles). Gate a plugin with its own `<prefix>.enabled = false` (config-driven,
`build` still runs and returns an inert variant) — or, for a compile-time gate,
`#[cfg(debug_assertions)] let b = b.plugin(DevReload);`.

### Same-typed beans

Qualifiers/named beans **do not exist** — there is no `name = "..."` on
`#[producer]` or `#[inject]`, and using one is a compile error. The bean graph is
keyed by type, so two beans of the same underlying type need two **newtypes**:

```rust
#[derive(Clone)] pub struct ReadPool(pub PgPool);
#[derive(Clone)] pub struct WritePool(pub PgPool);
// provide/produce/inject them like any other bean — `#[inject] pool: ReadPool`
```

A newtype is typed, compiler-checked, and works at **every** injection site
(controllers, beans, decorator beans, background services, plugins, guards).
