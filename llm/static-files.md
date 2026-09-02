---
topic: static-files
features: static
tokens: ~500
requires: plugins
---

## Static Files / SPA

### TL;DR

- Requires feature `static`; install with `.plugin(EmbeddedFrontend::new::<Assets>())` over a `rust_embed` asset type.
- Defaults are already on: SPA fallback, `api/` excluded, `assets/` immutable, ETag/304, range requests, `.br`/`.gz` negotiation.
- Install it AFTER the other router plugins — its SPA fallback is a Graph-stage effect applied in install order, so a later fallback would shadow it.
- Use `EmbeddedFrontend::builder::<Assets>()` to change prefixes, cache-control, or `base_path`; `.into_router()` is the escape hatch.

Requires feature: `static`. Serves `rust_embed` assets with SPA fallback,
ETag/304 conditional requests, range requests, and pre-compressed (`.br`/`.gz`)
variant negotiation — all on by default:

```rust
use r2e::r2e_static::EmbeddedFrontend;

# fn __doc(b: AppBuilder) -> impl Sized { b
.plugin(EmbeddedFrontend::new::<Assets>())        // SPA on, api/ excluded, assets/ immutable
# }
// its SPA fallback is a Graph-stage effect applied in install order — install it
// AFTER the other router plugins so no later fallback shadows it
// custom:
# fn __doc2() -> EmbeddedFrontend {
EmbeddedFrontend::builder::<Assets>()
    .spa_fallback(true)                            // serve index.html for unmatched routes
    .exclude_prefix("api/")                        // paths that should 404, not fall back
    .compression(true)                             // negotiate .br/.gz precompressed files
    .base_path("/docs")                            // mount under a sub-path (stripped on lookup)
    .immutable_prefix("assets/".to_string())       // long-lived immutable cache (takes Into<Option<String>>)
    .immutable_cache_control("public, max-age=31536000, immutable")
    .fallback_cache_control("no-cache")            // cache-control for the SPA fallback file
    .build()
# }
// escape hatch: EmbeddedFrontend::...build().into_router() -> r2e_core::http::Router
```
