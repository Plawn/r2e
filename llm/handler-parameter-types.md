---
topic: handler-parameter-types
features: core
tokens: ~900
requires: core-concepts
---

## Handler Parameter Types

### TL;DR

- Handler parameters come after `&self`; at most ONE body-consuming parameter (`Json`, `Form`, `String`, `Bytes`, `Multipart`, `TypedMultipart`, raw `Request`) and it MUST be the last one.
- Standard extractors: `Path<T>`, `Query<T>`, `Json<T>` (auto-validated), `Form<T>`, `HeaderMap`, `ConnectInfo<SocketAddr>`, `TypedMultipart<T>` (feature `multipart`).
- R2E-specific parameters: a `#[derive(Params)]` struct, `#[inject(identity)] user: AuthenticatedUser` (`Option<T>` = adaptive), `#[managed] tx: &mut Tx<'_, _>`, `SchedulerHandle`.
- Request-scoped data from a layer (`RequestId`, `RequestSpan`, `WorkerInfo`) is a plain, infallible parameter; for a field on the controller instead, use `#[inject(request)]` (llm/core-concepts.md).
- Use `peer: PeerAddr` — a plain parameter, NOT `#[inject(request)]` — for an infallible client-address read; it is `None` under in-process `TestApp`.

Alongside `&self`:

| Parameter | Description |
|-----------|-------------|
| `Path(id): Path<u64>` | URL path parameter |
| `Query(params): Query<T>` | Query string (T: Deserialize) |
| `Json(body): Json<T>` | JSON body (auto-validated if T: garde::Validate) |
| `body: Option<Json<T>>` | `None` when the request has no JSON content type; a malformed JSON body is still a 400 |
| `body: String` / `body: Bytes` | Raw body as UTF-8 text / bytes |
| `params: MyParams` | `#[derive(Params)]` aggregation (`#[param(path)]`/`#[query]`/`#[header]`; unattributed fields = query, `#[serde(rename…)]` honoured) |
| `#[inject(identity)] user: AuthenticatedUser` | Identity from JWT (`Option<T>` = adaptive) |
| `#[managed] tx: &mut Tx<'_, Sqlite>` | Managed resource (transaction) |
| `TypedMultipart(form): TypedMultipart<T>` | Typed multipart (feature `multipart`) |
| `multipart: Multipart` | Untyped streaming multipart (feature `multipart`; see llm/multipart.md) |
| `Form(data): Form<T>` | URL-encoded form |
| `HeaderMap` | All request headers |
| `MatchedPath` / `OriginalUri` | Route template that matched (`/users/{id}`) / URI before nesting stripped a prefix |
| `id: RequestId` | Request id (`.0: String`) from `RequestIdPlugin` / `HttpTrace`; a fresh UUID when neither set one |
| `span: RequestSpan` | The `HttpTrace` per-request span: `span.record("field", value)`; a no-op span on excluded paths (llm/observability.md) |
| `worker: WorkerInfo` | Worker serving the request (`id`, `workers`, `role`; control plane when not sharded) |
| `ConnectInfo(addr): ConnectInfo<SocketAddr>` | Client socket address |
| `peer: PeerAddr` | `PeerAddr(Option<SocketAddr>)` — infallible `ConnectInfo` read (`None` under in-process `TestApp`); plain param, not `#[inject(request)]` |
| `SchedulerHandle` | Scheduler control (when `Scheduler` installed) |
| `req: Request` | Raw request — MUST be the last parameter |

```rust
#[derive(serde::Deserialize)]
pub struct Patch { pub name: String }

#[controller(path = "/demo")]
pub struct DemoController;

#[routes]
impl DemoController {
    #[post("/{id}")]
    async fn update(
        &self,
        Path(id): Path<u64>,
        request_id: RequestId,
        matched: MatchedPath,
        body: Option<Json<Patch>>,          // body parameter last
    ) -> String {
        let name = body.map(|Json(p)| p.name).unwrap_or_default();
        format!("{id} {name} {} {}", request_id.0, matched.as_str())
    }

    #[post("/raw")]
    async fn raw(&self, body: Bytes) -> String {
        body.len().to_string()
    }
}
# fn main() {}
```
