# MCP Server (Model Context Protocol)

**Crate:** `r2e-mcp` · **Feature flag:** `mcp` (in `full`)

Expose tools to MCP clients (Claude, `npx @modelcontextprotocol/inspector`,
any agent) over MCP's **streamable-HTTP** transport, with the full R2E
runtime behind them: `#[inject]` beans, `#[config]`, guards, interceptors,
typed config, graceful shutdown, sharded serving, `TestApp`.

rmcp is used for the wire protocol only — **R2E dispatches tools itself**.
No rmcp type appears in user code (the one deliberate re-export is
`CallToolResult` for hand-built results).

## TL;DR

Expose MCP tools to agents (Claude, MCP Inspector) with the full R2E runtime behind them. `#[controller]` + `#[mcp_routes]` on a dedicated type turn plain async methods into tools: `#[tool(read_only)] async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut>` — the `schemars` schema of `AddIn` becomes the `inputSchema` (doc comments → descriptions), `Json<T>` returns are dual-encoded (`structuredContent` + text) and advertise an `outputSchema`. Guards and interceptors are the SAME machinery as HTTP (`#[guard(...)]` reads real transport headers; `#[intercept(...)]` specs are prebuilt from the bean graph), and a missing bean is a compile error at `.register_mcp_service::<T>()`. Setup is one line: `.plugin(McpServer::new())` mounts the streamable-HTTP endpoint at `/mcp` (config `mcp.*`), shares one session map across SO_REUSEPORT workers, and terminates live SSE streams on graceful shutdown. rmcp is the wire only — no rmcp type appears in user code. Requires feature `mcp`. Domain failures return `McpError::tool(...)` → `isError: true` results the agent can read; set `mcp.allowed-hosts` behind a proxy (loopback-only `Host` allowlist by default).

## Quick start

```rust
use r2e::prelude::*;
use schemars::JsonSchema;

#[derive(serde::Deserialize, JsonSchema)]
pub struct AddIn {
    /// Left operand.
    pub a: f64,
    /// Right operand.
    pub b: f64,
}

#[derive(serde::Serialize, JsonSchema)]
pub struct AddOut {
    pub value: f64,
}

#[controller]                       // same attribute as HTTP controllers
pub struct MathTools {
    #[inject] calc: CalcService,    // beans + #[config] fields work as usual
}

#[mcp_routes]
impl MathTools {
    /// Add two numbers.            // doc comment → tool description
    #[tool(read_only, idempotent)]
    async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut> {
        Json(AddOut { value: self.calc.add(p.a, p.b) })
    }
}
```

```rust
AppBuilder::new()
    .plugin(McpServer::new())               // mounts the endpoint at /mcp
    .provide(CalcService::default())
    .build_state().await
    .register_mcp_service::<MathTools>()    // compile-time bean check, like register_controller
    .serve_auto().await
```

Point the MCP Inspector at `http://localhost:3000/mcp`. A missing bean or a
bad `#[config]` key is caught at `register_mcp_service` — a missing
`.plugin(McpServer::new())` is a startup panic naming the fix.

## Tool signatures

`&self` plus **at most one of each**:

| parameter | meaning |
|---|---|
| `Params<T>` | Typed tool arguments. `T: Deserialize + JsonSchema`; the schema becomes the tool's `inputSchema` (doc comments → property descriptions, `Option<..>` fields → not required, nested types/enums kept as inline `$defs`). |
| `ToolCall` | Everything about the call: `arguments` (raw JSON), `parts` (HTTP request parts of the transport request — headers, URI, extensions), `request_id`, `cancel` (`CancelToken`, fired on client abort / shutdown). |
| `CancelToken` | Just the cancellation token. |
| `#[inject(identity)] user: I` / `Option<I>` | The authenticated caller (wired by the MCP auth layer — see the Auth section). |

Anything else is a targeted compile error — beans and config go on the
struct, not on methods.

Return types (`IntoToolResult`):

| return | wire result |
|---|---|
| `String` / `&str` | one text content block |
| `()` | empty success |
| `Json<T: Serialize + JsonSchema>` | dual-encoded: `structuredContent` + a JSON text block; the tool advertises an `outputSchema` |
| `CallToolResult` | passed through as-is |
| `Result<_, E: Into<McpError>>` | see error mapping below |

## `#[tool]` metadata

```rust
#[tool(name = "divide", title = "Division", read_only, destructive, idempotent, open_world)]
```

- `name` — wire name override (default: the method name). Tool names are
  **global across all registered services**; a duplicate is a boot panic
  naming both services.
- Doc comment → `description`, verbatim (paragraph breaks preserved).
- `read_only` / `destructive` / `idempotent` / `open_world` — advisory MCP
  annotations (clients use them for confirmation UX, never for security).

## Error mapping

`McpError::tool("division by zero")` produces a **successful** JSON-RPC
response with `result.isError = true` — the agent reads the message and can
retry with better arguments. Everything else maps to JSON-RPC errors:

| condition | JSON-RPC |
|---|---|
| arguments don't match `inputSchema` (validated before dispatch) | `-32602 invalid_params` |
| unknown tool name | `-32601 method_not_found` |
| guard rejection | `-32600` with the HTTP status text (`data: "forbidden"`, message carries the rejection body) |
| `McpError::Internal` / panics | `-32603 internal_error` |

## Guards and interceptors

Tools use the **same** `Guard<I>` / `Interceptor<R>` machinery as HTTP routes
— any `#[derive(DecoratorBean)]` guard or interceptor works unchanged, built
once at registration from the bean graph:

```rust
#[mcp_routes]
#[intercept(Logged::info())]          // impl-level: wraps every tool (outermost)
impl MathTools {
    #[tool(destructive)]
    #[guard(ApiKeyGuard)]             // reads real transport request headers
    #[intercept(Audit::spec("mcp"))]  // method-level (inner)
    async fn clear(&self) -> String { ... }
}
```

Guards receive a `GuardContext` with the HTTP headers/URI of the transport
request carrying the tool call. A guard's `#[inject]` dependencies are folded
into the service's `EndpointDeps` — a missing bean is a compile error at
`register_mcp_service`.

## One bean, two transports

`EndpointDeps` is one-per-type: a struct cannot carry both `#[routes]` and
`#[mcp_routes]`. The pattern is a shared bean plus thin adapters:

```rust
#[derive(Clone, Default)]
pub struct CalcService;              // the logic, plain bean

#[controller]
pub struct MathTools { #[inject] calc: CalcService }     // MCP adapter (#[mcp_routes])

#[controller(path = "/api/calc")]
pub struct CalcController { #[inject] calc: CalcService } // HTTP adapter (#[routes])
```

`examples/example-mcp` is the worked example (HTTP + MCP + guards +
interceptors + `TestApp` e2e tests).

## Configuration (`mcp.*`)

All keys optional; builder methods (`McpServer::new().with_path(...)` etc.)
take precedence over config.

```yaml
mcp:
  path: /mcp                  # literal path, no trailing slash / captures
  enabled: true               # false → endpoint not mounted (registration still compiles)
  name: my-server             # serverInfo.name    (default: CARGO_PKG_NAME)
  version: "1.2.3"            # serverInfo.version (default: CARGO_PKG_VERSION)
  instructions: "..."         # advertised usage instructions
  sse-keep-alive-secs: 15     # 0 disables keep-alive pings
  stateless: false            # true → no MCP sessions
  json-response: false        # true (stateless only) → plain application/json responses
  allowed-hosts: [api.example.com]   # DNS-rebinding protection — see below
  allowed-origins: []         # browser Origin allowlist
  max-request-body-bytes: 4194304
```

**`allowed-hosts` matters in deployment**: rmcp's default `Host` allowlist is
loopback-only, so an MCP endpoint behind a proxy or public hostname silently
403s every request until `mcp.allowed-hosts` names the public hostname(s).
R2E warns at boot when the bind host is non-loopback and the key is unset.

## Sharded serving & shutdown

- `server.workers: N` (SO_REUSEPORT): the session map, dispatch table and
  schema cache are built once and shared — a session initialized on one
  worker is usable from every worker.
- Graceful shutdown (`Ctrl-C`, `StopHandle::stop()`): a dedicated cancel
  token relayed from the app shutdown token terminates all live sessions and
  SSE streams, so drain never hangs on a long-lived stream.

## Testing

Over `TestApp` (in-process, no port): POST JSON-RPC envelopes to the endpoint
with `accept: application/json, text/event-stream` and (default rmcp host
protection) `host: localhost`. Responses are SSE `data:` events — skip the
empty priming event when parsing. Stateful mode requires the `initialize` →
`Mcp-Session-Id` header → `notifications/initialized` dance first;
`stateless: true` + `json-response: true` skips sessions entirely and answers
plain JSON. See `examples/example-mcp/tests/mcp_e2e.rs` for a complete
harness.

## Auth (upcoming)

The OAuth 2.1 resource-server layer (`mcp.auth.*`: issuer discovery, JWT
validation, RFC 9728 protected-resource metadata, static DCR shim for IdPs
without dynamic client registration — Keycloak, Google, Auth0, Entra, Okta)
is the next phase of this feature. `#[inject(identity)]` on tool parameters
is already wired to read the principal the auth layer deposits in request
extensions.
