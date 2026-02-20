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
- Configuration validation
- JWKS key pre-warming

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
    ↓
Process exit
```

If any `on_start` hook fails, execution stops immediately — later hooks and the server don't run.
