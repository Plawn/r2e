---
topic: method-attribute-quick-reference
features: core
tokens: ~500
requires: core-concepts
---

## Method Attribute Quick Reference

### TL;DR

- Routing markers: `#[get/post/put/delete/patch("/path")]`, `#[any("/path")]` (all methods, `{*path}` wildcards), `#[fallback]` (no path argument), `#[sse("/path")]`, `#[ws("/path")]`.
- Auth markers: `#[anonymous]` per route; `#[roles]`/`#[all_roles]`/`#[guard]`/`#[pre_guard]` on a method or on the impl — only `#[pre_guard]` also covers `#[anonymous]` routes.
- Cross-cutting markers: `#[intercept]`, `#[layer]`, `#[middleware]`, `#[managed]` (on a parameter), `#[async_exec]`.
- Off-request and documentation markers: `#[consumer]`, `#[scheduled]`, `#[request_helper]`, `#[status(N)]` / `#[returns(T)]`.

| Attribute | Level | Purpose |
|-----------|-------|---------|
| `#[get/post/put/delete/patch("/path")]` | method | Verb route |
| `#[any("/path")]` | method | All-methods route (wildcards: `{*path}`) |
| `#[fallback]` | method | App-wide catch-all (no path arg) |
| `#[sse("/path")]` | method | SSE endpoint |
| `#[ws("/path")]` | method | WebSocket endpoint |
| `#[anonymous]` | method | Skip struct-level identity for this route |
| `#[roles("r1", "r2")]` | method or impl | Any-of role check (403); impl = every non-anonymous route |
| `#[all_roles("r1", "r2")]` | method or impl | All-of role check (403); impl = every non-anonymous route |
| `#[guard(Expr)]` | method or impl | Post-auth guard (DecoratorSpec); impl = shared instance, runs before method guards |
| `#[pre_guard(Expr)]` | method or impl | Pre-auth guard (before JWT); impl = every route incl. `#[anonymous]` |
| `#[intercept(Expr)]` | method or impl | Interceptor |
| `#[managed]` | parameter | Managed resource lifecycle |
| `#[layer(expr)]` | method | Per-route Tower layer |
| `#[middleware(fn)]` | method | Per-route middleware fn |
| `#[consumer(bus = "field")]` | method | Event consumer |
| `#[scheduled(every = "5m")]` | method | Scheduled task |
| `#[async_exec]` | method (bean or controller) | Submit body to `PoolExecutor`, returns `JobHandle` |
| `#[request_helper]` | method | Helper on the per-request façade (reads identity; callable only from routes/SSE/WS) |
| `#[status(200)]` / `#[returns(T)]` | method | OpenAPI overrides |
