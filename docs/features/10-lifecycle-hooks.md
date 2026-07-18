# Feature 10 — Lifecycle Hooks

## TL;DR

Run code at server startup and shutdown. `.on_start(|state| async move { ... })` fires before the server accepts connections and may return `Err` to abort boot; `.on_stop(|state| async move { ... })` fires after graceful shutdown and cannot fail. Both are registered after `build_state()` and receive the inferred bean HList — read beans by type with `state.bean::<T>()` (`BeanLookup`). For drain-time hooks and programmatic stop, see Feature 22 (`on_drain`, `StopHandle`).


## Goal

Allow executing code at server startup and shutdown, to initialize resources or perform cleanup.

## Key Concepts

### on_start

Hook executed **before** the server starts listening for connections. Receives the application state as a parameter. Can return an error to prevent startup.

### on_stop

Hook executed **after** the server's graceful shutdown (Ctrl+C signal, SIGTERM, or a programmatic `StopHandle::stop()`). Receives the application state, cannot fail.

### on_drain / StopHandle

Two more lifecycle pieces live in [Feature 22 — Serve Lifecycle](./22-serve-lifecycle.md): `on_drain` hooks are awaited when shutdown is *triggered*, **before** the server stops accepting connections (readiness flip, load-balancer deregistration), and `StopHandle` stops a running server programmatically through the same graceful path.

## Usage

### 1. Startup Hook

Hooks are registered on the built phase, after `.build_state().await`. The state is the inferred bean HList — read beans from it by type with `state.bean::<T>()` (`BeanLookup`, in the prelude):

```rust
AppBuilder::new()
    .provide(pool)
    .build_state()
    .await
    .on_start(|state| async move {
        // Verifier la connexion a la base de donnees
        let pool = state.bean::<sqlx::SqlitePool>().expect("pool bean");
        sqlx::query("SELECT 1").execute(&pool).await?;
        tracing::info!("Connexion DB verifiee");

        // Initialiser des donnees
        tracing::info!("Application demarree");
        Ok(())
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### Startup Hook Signature

```rust
FnOnce(T) -> Future<Output = Result<(), Box<dyn Error + Send + Sync>>>
```

- Receives `T` (the application state, cloned) — the inferred HList of resolved beans; access individual beans with `state.bean::<T>()`
- Must return `Ok(())` to allow startup to proceed
- If it returns `Err(...)`, the server does not start and the error is propagated

### Example: Seeding an admin user from environment variables

A common pattern is to create an initial admin user at startup. `on_start` gives access to the full state (the resolved bean graph), and `R2eConfig` — itself a bean in the graph — automatically overlays environment variables (`ADMIN_EMAIL` -> `admin.email`) — no need for `std::env::var()` or manually reconstructing repositories:

```rust
AppBuilder::new()
    .load_config::<RootConfig>()
    .register::<UserRepo>()
    .build_state()
    .await
    .on_start(|state| async move {
        // R2eConfig mappe ADMIN_EMAIL → admin.email automatiquement
        let config = state.bean::<R2eConfig>().expect("config bean");
        let email: String = config.get("admin.email").unwrap_or_default();
        let password: String = config.get("admin.password").unwrap_or_default();

        if email.is_empty() || password.is_empty() {
            return Ok(()); // pas de seed demande
        }

        // UserRepo est deja dans le graphe via DI
        let user_repo = state.bean::<UserRepo>().expect("user repo bean");
        if user_repo.find_by_email(&email).await?.is_some() {
            tracing::debug!("Admin seed skipped — {} already exists", email);
            return Ok(());
        }

        let hash = hash_password(&password)?;
        user_repo.create(&NewUser {
            email: email.clone(),
            role: Role::Admin,
            password_hash: Some(hash),
            ..Default::default()
        }).await?;

        tracing::info!("Admin user seeded: {}", email);
        Ok(())
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

Key points:
- **No `std::env::var()`** — use `R2eConfig` (a bean: `state.bean::<R2eConfig>()`). Environment variables are automatically mapped (`ADMIN_EMAIL` -> `admin.email`).
- **No manual repository construction** — beans are already resolved in the graph; fetch them by type with `state.bean::<T>()`.
- **Use `?` for error propagation** — `on_start` returns `Result`, so errors cleanly block startup instead of being silently logged.

### 2. Shutdown Hook

```rust
AppBuilder::new()
    .build_state()
    .await
    .on_stop(|_state| async {
        tracing::info!("Arret en cours...");
        // Nettoyage, flush des logs, fermeture de connexions...
        tracing::info!("Nettoyage termine");
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

### Shutdown Hook Signature

```rust
FnOnce(T) -> Future<Output = ()>
```

- Receives the application state (same signature as `on_start`)
- Cannot fail (returns `()`)

### 3. Multiple Hooks

Both methods can be called multiple times. Hooks are executed in registration order:

```rust
AppBuilder::new()
    .provide(pool)
    .build_state()
    .await
    .on_start(|state| async move {
        tracing::info!("Hook 1 : verification DB");
        let pool = state.bean::<sqlx::SqlitePool>().expect("pool bean");
        sqlx::query("SELECT 1").execute(&pool).await?;
        Ok(())
    })
    .on_start(|_state| async move {
        tracing::info!("Hook 2 : chargement cache");
        Ok(())
    })
    .on_stop(|_state| async {
        tracing::info!("Hook arret 1");
    })
    .on_stop(|_state| async {
        tracing::info!("Hook arret 2");
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

## Shutdown Grace Period

By default, R2E waits indefinitely for shutdown hooks to complete. Use `shutdown_grace_period` to set a maximum delay — if hooks do not finish within the allotted time, the process forces shutdown:

```rust
use std::time::Duration;

AppBuilder::new()
    .build_state()
    .await
    .shutdown_grace_period(Duration::from_secs(5))
    .on_stop(|_state| async {
        tracing::info!("Nettoyage...");
    })
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

This replaces the common pattern where users manually spawn a shutdown handler with `CancellationToken` + `tokio::signal::ctrl_c()` + `process::exit()`.

## Execution Order

```
1. on_start hooks (sequential, in registration order)
2. Server starts listening (TCP bind)
3. ... request processing ...
4. Shutdown signal received (Ctrl+C / SIGTERM)
5. Graceful server shutdown
6. on_stop hooks (sequential, in registration order)
7. If grace period is defined and exceeded -> force exit
```

### Startup Hook Failure

If an `on_start` hook returns `Err`, execution stops immediately:
- Subsequent hooks are **not** executed
- The server does **not** start listening
- The error is propagated to the caller of `serve()`

## Typical Use Cases

### Startup

| Use Case | Example |
|----------|---------|
| Connectivity check | Test the DB connection before accepting requests |
| Schema migration | Run migrations at startup |
| Data seeding | Create an initial admin from environment variables |
| Cache loading | Pre-populate an in-memory cache |
| Configuration check | Validate that all required keys are present |
| Informational logging | Display the version, active profile, etc. |

### Shutdown

| Use Case | Example |
|----------|---------|
| Log/metrics flush | Send remaining metrics before shutdown |
| Connection closing | Cleanly close external connections |
| Notification | Notify a monitoring system of the shutdown |
| State persistence | Persist in-memory state to disk |

## LifecycleController Trait

For more advanced cases, the `LifecycleController` trait allows defining hooks directly on a controller. The state type is inferred (HList), so the impl is **generic over the state** — bound it with `BeanLookup` if the hook reads beans:

```rust
impl<S: BeanLookup + Clone + Send + Sync + 'static> LifecycleController<S> for MyController {
    fn on_start(state: &S) -> Pin<Box<dyn Future<Output = Result<...>> + Send + '_>> {
        Box::pin(async move {
            tracing::info!("MyController starting");
            Ok(())
        })
    }

    fn on_stop(_state: &S) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {
            tracing::info!("MyController stopping");
        })
    }
}
```

## Bean & Controller Hooks: `#[post_construct]` / `#[pre_destroy]`

Besides the app-level `on_start`/`on_stop` closures, R2E provides per-bean and
per-controller lifecycle hooks as method attributes. Both work on `#[bean]` methods
**and** on `#[routes]` controller impls. The method takes `&self` only (no params), may be
`async`, and returns `()` or `Result<(), Box<dyn Error + Send + Sync>>`.

### `#[post_construct]`

The `@PostConstruct` counterpart — runs after the instance is built.

- **Bean hooks** run inside `build_state()`, after the graph resolves and before subscribers.
- **Controller-core hooks** run at startup during `register_controller`, **before** consumer
  registrations (later than bean hooks, since cores are built after the graph).
- An `Err` **aborts startup**.

```rust
#[routes]
impl UserController {
    #[post_construct]
    async fn warm_cache(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.cache.prime().await?;
        Ok(())
    }
}
```

### `#[pre_destroy]`

The `@PreDestroy` counterpart — runs at **graceful shutdown**, in the async shutdown phase:
controller hooks first, then bean hooks, each in **reverse registration order**. An `Err` is
**logged and swallowed** (it never aborts shutdown). It does **not** fire on
`build_with_consumers`/`TestApp` (no shutdown occurs) — test it via `serve` + `StopHandle::stop()`.

Combining `#[post_construct]` or `#[pre_destroy]` with a route / `#[scheduled]` / `#[consumer]`
marker, with method params, or with `#[intercept]` is a compile error.

## Validation Criteria

```bash
cargo run -p example-app
```

At startup:

```
INFO "R2E example-app startup hook executed"
INFO addr="0.0.0.0:3000" "R2E server listening"
```

At shutdown (Ctrl+C):

```
INFO "Shutdown signal received, starting graceful shutdown"
INFO "R2E example-app shutdown hook executed"
INFO "R2E server stopped"
```
