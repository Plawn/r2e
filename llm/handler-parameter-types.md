---
topic: handler-parameter-types
features: core
tokens: ~400
requires: core-concepts
---

## Handler Parameter Types

### TL;DR

- Handler parameters come after `&self`; a raw `req: Request` MUST be the last parameter.
- Standard extractors: `Path<T>`, `Query<T>`, `Json<T>` (auto-validated), `Form<T>`, `HeaderMap`, `ConnectInfo<SocketAddr>`, `TypedMultipart<T>` (feature `multipart`).
- R2E-specific parameters: a `#[derive(Params)]` struct, `#[inject(identity)] user: AuthenticatedUser` (`Option<T>` = adaptive), `#[managed] tx: &mut Tx<'_, _>`, `SchedulerHandle`.
- Use `peer: PeerAddr` — a plain parameter, NOT `#[inject(request)]` — for an infallible client-address read; it is `None` under in-process `TestApp`.

Alongside `&self`:

| Parameter | Description |
|-----------|-------------|
| `Path(id): Path<u64>` | URL path parameter |
| `Query(params): Query<T>` | Query string (T: Deserialize) |
| `Json(body): Json<T>` | JSON body (auto-validated if T: garde::Validate) |
| `params: MyParams` | `#[derive(Params)]` aggregation (`#[param(path)]`/`#[query]`/`#[header]`; unattributed fields = query, `#[serde(rename…)]` honoured) |
| `#[inject(identity)] user: AuthenticatedUser` | Identity from JWT (`Option<T>` = adaptive) |
| `#[managed] tx: &mut Tx<'_, Sqlite>` | Managed resource (transaction) |
| `TypedMultipart(form): TypedMultipart<T>` | Typed multipart (feature `multipart`) |
| `Form(data): Form<T>` | URL-encoded form |
| `HeaderMap` | All request headers |
| `ConnectInfo(addr): ConnectInfo<SocketAddr>` | Client socket address |
| `peer: PeerAddr` | `PeerAddr(Option<SocketAddr>)` — infallible `ConnectInfo` read (`None` under in-process `TestApp`); plain param, not `#[inject(request)]` |
| `SchedulerHandle` | Scheduler control (when `Scheduler` installed) |
| `req: Request` | Raw request — MUST be the last parameter |
