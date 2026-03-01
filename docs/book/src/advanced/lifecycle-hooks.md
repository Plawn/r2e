# Lifecycle Hooks

R2E provides `on_start` and `on_stop` hooks for running code during application startup and shutdown.

## `on_start` — Startup hook

Runs before the server starts listening. Receives the application state:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .on_start(|state| async move {
        // Verify database connectivity
        sqlx::query("SELECT 1").execute(&state.pool).await?;
        tracing::info!("Database connection verified");
        Ok(())
    })
    // ...
```

### Failure handling

`on_start` returns `Result<(), Box<dyn Error>>`. If any startup hook fails, the application terminates immediately — the server never binds:

```rust
.on_start(|state| async move {
    if !state.config.get::<bool>("app.ready").unwrap_or(false) {
        return Err("Application not ready".into());
    }
    Ok(())
})
```

### Common use cases

- Database connectivity checks
- Running migrations (`sqlx::migrate!().run(&pool).await`)
- Cache preloading
- Data seeding
- Configuration validation
- JWKS key pre-warming

### Example: seeding an admin user from environment variables

A common pattern is seeding an initial admin user at startup. `on_start` gives you full access to the DI-resolved state, and `R2eConfig` automatically overlays environment variables (`ADMIN_EMAIL` → `admin.email`), so there's no need for manual `std::env::var()` calls or reconstructing repositories:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .on_start(|state| async move {
        // R2eConfig maps ADMIN_EMAIL → admin.email automatically
        let email: String = state.config.get("admin.email").unwrap_or_default();
        let password: String = state.config.get("admin.password").unwrap_or_default();

        if email.is_empty() || password.is_empty() {
            return Ok(()); // no seed requested
        }

        // user_repo is already in the state via DI — no manual construction
        if state.user_repo.find_by_email(&email).await?.is_some() {
            tracing::debug!("Admin seed skipped — {} already exists", email);
            return Ok(());
        }

        let hash = hash_password(&password)?;
        state.user_repo.create(&NewUser {
            email: email.clone(),
            role: Role::Admin,
            password_hash: Some(hash),
            ..Default::default()
        }).await?;

        tracing::info!("Admin user seeded: {}", email);
        Ok(())
    })
    .serve("0.0.0.0:3000").await
```

Key points:
- **No `std::env::var()`** — use `state.config.get()` instead. Environment variables are overlaid automatically (`ADMIN_EMAIL` → `admin.email`).
- **No manual repository construction** — services are already available in the state via DI.
- **Use `?` for error propagation** — `on_start` returns `Result`, so errors block server startup cleanly instead of being silently logged.

## `on_stop` — Shutdown hook

Runs after the server stops accepting connections and all in-flight requests complete:

```rust
.on_stop(|| async {
    tracing::info!("Shutdown in progress");
    // Flush logs, close connections, notify monitoring
})
```

Shutdown hooks don't return `Result` — there's no meaningful way to handle errors during shutdown.

### Common use cases

- Flushing metrics/logs
- Closing external connections
- Notifying monitoring systems
- Saving in-memory state

## Multiple hooks

Register multiple hooks — they execute in registration order:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .on_start(|state| async move {
        tracing::info!("Hook 1: check DB");
        Ok(())
    })
    .on_start(|state| async move {
        tracing::info!("Hook 2: warm cache");
        Ok(())
    })
    .on_stop(|| async {
        tracing::info!("Hook 1: flush logs");
    })
    .on_stop(|| async {
        tracing::info!("Hook 2: close connections");
    })
    // ...
```

## Shutdown grace period

By default, R2E waits indefinitely for shutdown hooks to complete. Use `shutdown_grace_period` to set a maximum duration — if hooks don't finish in time, the process force-exits:

```rust
use std::time::Duration;

AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .shutdown_grace_period(Duration::from_secs(5))
    .on_stop(|| async {
        tracing::info!("Cleaning up...");
    })
    .serve("0.0.0.0:3000").await
```

This replaces the common pattern of manually spawning a shutdown handler with `CancellationToken` + `tokio::signal::ctrl_c()` + `process::exit()`.

## Execution sequence

```
on_start hooks (in order)
    ↓ (if all succeed)
TCP bind
    ↓
Serve requests
    ↓ (Ctrl-C / SIGTERM)
Stop accepting connections
    ↓
Wait for in-flight requests
    ↓
on_stop hooks (in order)
    ↓ (if grace period set and exceeded → force exit)
Process exit
```

If any `on_start` hook fails, execution stops immediately — later hooks and the server don't run.
