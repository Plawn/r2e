# r2e-static

Embedded static file serving with SPA support for [R2E](https://github.com/plawn/r2e).

## Overview

Serves frontend assets embedded in the binary via [`rust_embed`](https://crates.io/crates/rust-embed), with automatic MIME detection, cache headers, ETag support, pre-compressed variants, range requests, and SPA fallback.

## Quick start

```rust
use r2e_static::EmbeddedFrontend;

#[derive(rust_embed::Embed)]
#[folder = "frontend/dist"]
struct Assets;

// In your AppBuilder chain:
app.with(EmbeddedFrontend::new::<Assets>())
```

Defaults: SPA fallback on, `api/` excluded, `assets/` gets immutable cache headers, compression on.

## Builder API

```rust
app.with(EmbeddedFrontend::builder::<Assets>()
    .exclude_prefix("api/")
    .exclude_prefix("graphql/")
    .immutable_prefix(Some("assets/".into()))
    .spa_fallback(true)
    .fallback_file("index.html")
    .compression(true)
    .build())
```

Mount on a sub-path (no SPA):

```rust
app.with(EmbeddedFrontend::builder::<DocsAssets>()
    .spa_fallback(false)
    .base_path("/docs")
    .build())
```

## Features

- Exact file match with MIME detection via `mime_guess`
- Directory index (`/` and `foo/` serve `index.html`)
- SPA fallback (unknown routes serve `index.html`)
- Excluded prefixes (default: `api/`)
- Immutable cache headers for hashed assets (default: `assets/`)
- ETag headers from `rust_embed` SHA-256 hashes, with `304 Not Modified` on `If-None-Match`
- Pre-compressed variants (`.br`/`.gz`) served based on `Accept-Encoding` (default: on)
- Range requests (`206 Partial Content`) for byte-range requests
- Base path mounting for sub-path serving
- Installs as an Axum fallback handler via the R2E plugin system, or nest under a sub-path with `into_router()`

## License

Apache-2.0
