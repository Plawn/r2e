# Dev Mode

R2E provides development-mode endpoints for hot-reload detection and diagnostics.

## Enabling dev mode

Add the `DevReload` plugin:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(DevReload)
    // ...
```

## Dev endpoints

### Status

```
GET /__r2e_dev/status → "dev"
```

Returns plain text `"dev"`. Use to check if the server is running in development mode.

### Ping

```
GET /__r2e_dev/ping → {"boot_time": 1234567890123, "status": "ok"}
```

Returns the server's boot timestamp (milliseconds since epoch). Use to detect server restarts.

## Hot-reload workflow

1. Use `cargo watch` (or `r2e dev`) to auto-restart on file changes
2. Client-side JavaScript polls `/__r2e_dev/ping`
3. When `boot_time` changes, a restart occurred → refresh the page

```
Source code change
    → cargo-watch detects change
    → kills server process
    → rebuilds and restarts
    → new boot_time
    → client detects → page refresh
```

### Client-side polling example

```javascript
let lastBootTime = null;

setInterval(async () => {
    try {
        const res = await fetch('/__r2e_dev/ping');
        const data = await res.json();

        if (lastBootTime === null) {
            lastBootTime = data.boot_time;
        } else if (data.boot_time !== lastBootTime) {
            location.reload();
        }
    } catch {
        // Server is restarting, wait for next poll
    }
}, 1000);
```

## Using `r2e dev`

The CLI provides a convenient wrapper:

```bash
r2e dev
```

This:
- Sets `R2E_PROFILE=dev`
- Watches `src/`, `application.yaml`, `application-dev.yaml`, `migrations/`
- Prints discovered routes
- Auto-restarts on changes

Add `--open` to auto-open the browser:

```bash
r2e dev --open
```

## Production note

Do **not** enable `DevReload` in production. The dev endpoints are informational only but expose internal details (boot time) that shouldn't be public.

```rust
#[cfg(debug_assertions)]
builder = builder.with(DevReload);
```
