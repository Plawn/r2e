---
topic: method-attribute-quick-reference
features: core
tokens: ~900
requires: core-concepts
---

## Method Attribute Quick Reference

### TL;DR

- Routing markers: `#[get/post/put/delete/patch("/path")]`, `#[any("/path")]` (all methods, `{*path}` wildcards), `#[fallback]` (no path argument), `#[sse("/path")]`, `#[ws("/path")]`.
- Auth markers: `#[anonymous]` per route; `#[roles]`/`#[all_roles]`/`#[guard]`/`#[pre_guard]` on a method or on the impl — only `#[pre_guard]` also covers `#[anonymous]` routes.
- Cross-cutting markers: `#[intercept]`, `#[layer]`, `#[middleware]` (all three on a method or on the impl), `#[managed]` (on a parameter), `#[async_exec]`.
- Off-request and documentation markers: `#[consumer]`, `#[scheduled]`, `#[request_helper]`, `#[status(N)]` / `#[returns(T)]`.
- MCP members (`#[tool]`, `#[resource(uri = …)]`, `#[prompt]`, `#[completion]`, see llm/mcp-server.md) make a `#[routes]` impl an MCP service. One type = one port: an MCP member next to an HTTP route in the same impl is a compile error — write one `#[controller]` type per port over the shared beans. Impl-level `#[roles]`/`#[all_roles]`/`#[guard]`/`#[intercept]` cover MCP members too (decorators built once per registration, over the type's one core); `#[pre_guard]`/`#[layer]`/`#[middleware]` are HTTP-only (a compile error on an MCP service).

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
| `#[layer(expr)]` / `#[layer(\|this: &Self\| expr)]` | method or impl | Tower layer; closure form builds it from the controller core once at registration; impl = every route except `#[fallback]`, outside method layers |
| `#[middleware(fn)]` / `#[middleware(fn, state)]` | method or impl | Middleware fn `async fn(Request, Next) -> Response`; `state` passes the core as a leading `Arc<Self>` argument; impl = every route except `#[fallback]` |
| `#[consumer(bus = "field")]` | method | Event consumer |
| `#[scheduled(every = "5m")]` / `#[scheduled(schedule = "method")]` | method | Scheduled task; `schedule` names a sync `&self -> ScheduleConfig` method on the same impl, called once at registration (cadence from `#[config]` / a bean) |
| `#[async_exec]` | method (bean or controller) | Submit body to `PoolExecutor`, returns `JobHandle` |
| `#[request_helper]` | method | Helper on the per-request façade (reads identity; callable only from routes/SSE/WS) |
| `#[status(200)]` / `#[returns(T)]` | method | OpenAPI overrides |
| `#[tool]` / `#[resource(uri)]` / `#[prompt]` / `#[completion]` | method | MCP member on a `#[routes]` impl (served by `register_mcp_service`) |
| `#[routes(group = "g", opt_in)]` | impl | MCP group of the impl's MCP members (compile error without one) |
