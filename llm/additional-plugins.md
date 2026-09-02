---
topic: additional-plugins
features: core
tokens: ~200
requires: plugins
---

## Additional Plugins

### TL;DR

- Install each of these with `.plugin(..)` during the builder phase, like any other
  plugin. See llm/plugins.md.
- `RequestIdPlugin` — X-Request-Id propagation. `HttpTrace` already does this
  (llm/observability.md); installing both, in either order, is harmless.
- `SecureHeaders::default()` — security headers.
- `Health` — `/health`; use `Health::builder()` instead when you need live/ready probes.
- `NormalizePath` — trailing-slash normalization; its install order is irrelevant.

```rust
# fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(RequestIdPlugin)                   // X-Request-Id propagation (HttpTrace also does it)
.plugin(SecureHeaders::default())          // security headers
.plugin(Health)                            // /health, or Health::builder() for live/ready probes
.plugin(NormalizePath)                     // trailing-slash normalization (order irrelevant)
# }
```
