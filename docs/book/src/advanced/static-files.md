# Static Files

R2E can serve static files embedded directly in your binary using the `r2e-static` crate. The `EmbeddedFrontend` plugin wraps [rust_embed](https://docs.rs/rust-embed) and provides SPA fallback support, cache control headers, and prefix-based routing out of the box.

## Quick start

Define your asset type with `rust_embed`, then install the plugin:

```rust
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct Assets;
```

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(EmbeddedFrontend::new::<Assets>())
    .serve("0.0.0.0:3000")
    .await;
```

With the defaults, this gives you:

- SPA fallback enabled (unmatched routes serve `index.html`)
- `api/` prefix excluded (API routes pass through normally)
- Files under `assets/` served with immutable cache headers
- All other files served with a 1-hour cache

## Builder pattern

Use `EmbeddedFrontend::builder()` for full control:

```rust
AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(
        EmbeddedFrontend::builder::<Assets>()
            .spa_fallback(true)
            .fallback_file("index.html")
            .exclude_prefix("api/")
            .exclude_prefix("ws/")
            .immutable_prefix(Some("assets/".to_string()))
            .immutable_cache_control("public, max-age=31536000, immutable")
            .default_cache_control("public, max-age=3600")
            .base_path("/app")
            .build()
    )
    .serve("0.0.0.0:3000")
    .await;
```

### Builder methods

| Method | Default | Description |
|--------|---------|-------------|
| `spa_fallback(bool)` | `true` | When enabled, requests that don't match any file are served the fallback file instead of returning 404. |
| `fallback_file(impl Into<String>)` | `"index.html"` | The file served for unmatched routes in SPA mode. |
| `exclude_prefix(impl Into<String>)` | `"api/"` | Adds a path prefix that should bypass static serving and return 404. Call multiple times to add more. |
| `clear_excluded_prefixes()` | -- | Removes all excluded prefixes, including the default `api/`. |
| `immutable_prefix(impl Into<Option<String>>)` | `Some("assets/")` | Files under this prefix receive immutable cache headers. Pass `None` to disable. |
| `immutable_cache_control(impl Into<String>)` | `"public, max-age=31536000, immutable"` | The `Cache-Control` header value for files matching the immutable prefix. |
| `default_cache_control(impl Into<String>)` | `"public, max-age=3600"` | The `Cache-Control` header value for all other files. |
| `base_path(impl Into<String>)` | None | Mount static files under a sub-path (e.g. `"/docs"`). The base path is stripped before looking up files. |

## How it works

`EmbeddedFrontend` installs itself as a **fallback handler** on the Axum router. This means your API routes always take priority -- the static file handler only runs when no other route matches.

The request resolution order is:

1. **Excluded prefixes** -- if the path starts with an excluded prefix (e.g. `api/`), return 404 immediately.
2. **Exact file match** -- look up the path in the embedded assets.
3. **Directory index** -- for paths ending in `/` (or the root), try appending `index.html`.
4. **SPA fallback** -- if enabled, serve the fallback file (default `index.html`).
5. **404** -- nothing matched.

When a file is found, the response includes:

- A `Content-Type` header inferred from the file extension
- A `Cache-Control` header (immutable or default, depending on the path)
- An `ETag` header derived from the file's SHA-256 hash

## rust_embed setup

Add the dependencies to your `Cargo.toml`:

```toml
[dependencies]
r2e-static = { path = "../r2e-static" }
rust-embed = "8"
```

Point the `#[folder]` attribute at your frontend build output:

```rust
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct Assets;
```

In debug mode, `rust_embed` reads files from disk (so you get live reloading of assets). In release mode, files are compiled into the binary.

## Mounting under a sub-path

Use `base_path` to serve your frontend from a sub-path. The base path is stripped before looking up files in the embedded assets:

```rust
EmbeddedFrontend::builder::<Assets>()
    .base_path("/app")
    .build()
```

With this configuration:
- `GET /app/` serves `index.html`
- `GET /app/assets/main.js` serves `assets/main.js`
- `GET /other` returns 404 (does not match the base path)

## Disabling SPA fallback

For plain static file serving without SPA behavior:

```rust
EmbeddedFrontend::builder::<Assets>()
    .spa_fallback(false)
    .build()
```

Requests for non-existent files will return 404 instead of falling back to `index.html`.

## Complete example

```rust
use r2e::prelude::*;
use r2e_static::EmbeddedFrontend;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct Assets;

#[tokio::main]
async fn main() {
    AppBuilder::new()
        .build_state::<AppState, _, _>()
        .await
        .with(Tracing)
        .with(Health)
        .with(
            EmbeddedFrontend::builder::<Assets>()
                .exclude_prefix("api/")
                .exclude_prefix("ws/")
                .immutable_prefix(Some("assets/".to_string()))
                .spa_fallback(true)
                .build()
        )
        .serve("0.0.0.0:3000")
        .await;
}
```

> **Note:** `EmbeddedFrontend` marks itself with `should_be_last() = true`. R2E will warn if you install other plugins after it, since it registers a fallback handler that would shadow later fallbacks.
