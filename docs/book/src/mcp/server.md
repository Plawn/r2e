# MCP Server

R2E can expose your beans to LLM agents as a [Model Context Protocol](https://modelcontextprotocol.io/)
server. Tools, resources and prompts are members of a `#[routes]` impl, exactly
like HTTP routes, and reuse the same guards, interceptors and identity.

## Setup

```toml
r2e = { version = "0.15", features = ["mcp"] }
schemars = "1"
```

`mcp` is part of `full`. Two extra features are opt-in:

| Feature | Adds |
|---|---|
| `mcp-testing` | The in-process test client (`r2e::r2e_mcp::testing`) |
| `mcp-loopback` | `McpLoopback`, an in-process transport on the same endpoint |

## A first tool

```rust
use r2e::prelude::*;
use schemars::JsonSchema;

#[derive(serde::Deserialize, JsonSchema, ObjectParams)]
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

#[controller]
pub struct MathTools {
    #[inject]
    calc: CalcService,
}

#[routes]
impl MathTools {
    /// Add two numbers.
    #[tool(read_only, idempotent)]
    async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut> {
        Json(AddOut { value: self.calc.add(p.a, p.b) })
    }
}
```

Register the plugin and the service:

```rust
AppBuilder::new()
    .plugin(McpServer::new())
    .provide(CalcService::default())
    .build_state()
    .await
    .register_mcp_service::<MathTools>()
    .serve_auto()
    .await
```

The doc comment becomes the tool description and the field doc comments end up
in the input schema. The endpoint is served at `/mcp` (streamable HTTP).

### `#[derive(ObjectParams)]`

A `Params<T>` type needs three derives: `Deserialize`, `JsonSchema` and
`ObjectParams`. `ObjectParams` is a marker saying `T` is a valid MCP argument
object. The derive only accepts named-field structs: an enum, a tuple struct or
a unit struct is a compile error, so `Params<u32>` or `Params<MyEnum>` cannot
produce an invalid `inputSchema`. If you forget the derive, the error is a
trait-bound error on the tool method.

**One type, one port.** A `#[routes]` impl holds either HTTP routes or MCP
members, never both. To expose the same logic over HTTP and MCP, write two
`#[controller]` types over the same beans. See `examples/example-mcp`.

## Tools

`#[tool(...)]` options:

| Option | Meaning |
|---|---|
| `name = "..."`, `title = "..."` | Override the method name, set a display title |
| `read_only`, `destructive`, `idempotent`, `open_world` | MCP tool annotations |
| `meta = meta!({ .. })` | Free-form `_meta` object |
| `scopes = [..]` | OAuth scopes required when `mcp.auth` is enabled |

Parameters (at most one of each):

| Parameter | Purpose |
|---|---|
| `Params<T>` | Deserialized arguments; `T` drives the input schema |
| `ToolCall` | Raw call context (name, raw arguments) |
| `CancelToken` | Cancelled when the client cancels the request |
| `McpSession` | The current session |
| `Progress` | Progress notifications |
| `McpClient<'_>` | Calls back into the client (elicitation, …) |

Return types: `String` / `&str`, `()`, `Json<T>` (sets `structuredContent` and
an `outputSchema`), `CallToolResult`, or `Result<_, E>` where `E: Into<McpError>`.

### Errors

`McpError::tool("message")` produces a tool result with `isError: true`, which
the model sees and can react to. Protocol failures map to JSON-RPC codes:
`-32602` invalid params, `-32601` unknown tool, `-32600` guard rejection,
`-32603` internal error. `McpError::rpc(code, message, data)` sets your own.

## Resources and prompts

```rust
#[routes]
impl UserResources {
    /// A user profile.
    #[resource(uri = "r2e://users/{id}", mime_type = "application/json")]
    async fn user(&self, call: ResourceCall) -> Result<Json<User>, McpError> {
        let id = &call.variables["id"];
        self.users.find(id).await.map(Json).ok_or_else(|| McpError::tool("not found"))
    }

    /// Summarize a document.
    #[prompt(name = "summarize")]
    async fn summarize(&self, Params(p): Params<SummarizeIn>) -> String {
        format!("Summarize the following text:\n\n{}", p.text)
    }
}
```

Other members: `#[completion]` providers for argument completion, and the
`McpResourceUpdates` bean, whose `notify(uri)` pushes `resources/updated` to
subscribed clients.

## Guards and identity

MCP members use the same decorators as HTTP: `#[guard(..)]`, `#[roles(..)]`,
`#[intercept(..)]`, and `#[inject(identity)]` on the struct or as a parameter.
Authentication is fail-closed as for HTTP controllers.

To make the endpoint an OAuth 2.1 resource server, set `server.public-url` and
`mcp.auth.issuer`. R2E then serves the protected-resource metadata and validates
bearer tokens.

## Configuration (`mcp.*`)

| Key | Default | Meaning |
|---|---|---|
| `mcp.enabled` | `true` | Turn the endpoint off |
| `mcp.path` | `/mcp` | Endpoint path |
| `mcp.name`, `mcp.version`, `mcp.instructions` | crate info | Server info sent at `initialize` |
| `mcp.stateless` | `false` | No sessions |
| `mcp.json-response` | `false` | Plain JSON responses instead of SSE |
| `mcp.sse-keep-alive-secs` | `15` | SSE keep-alive interval |
| `mcp.elicitation-timeout-secs` | `300` | Wait for an elicitation answer |
| `mcp.page-size` | unset (one page) | Page size for `*/list` |
| `mcp.session.idle-timeout-secs` | `300` | Close idle sessions |
| `mcp.session.store-ttl-secs` | `1800` | Session store retention |
| `mcp.allowed-hosts` | loopback only | `Host` allow-list (DNS-rebinding guard) |
| `mcp.allowed-origins` | — | `Origin` allow-list |
| `mcp.max-request-body-bytes` | — | Request size limit |

> **Behind a proxy**, set `mcp.allowed-hosts` to your public host name: the
> default only accepts loopback hosts.

Sessions work with sharded serving, and graceful shutdown closes open SSE
streams.

## Testing

Enable `mcp-testing` in `[dev-dependencies]`, then:

```rust
use r2e::r2e_mcp::testing::McpTestExt;

#[r2e::test(app = McpApp)]
async fn adds(app: TestApp) {
    let mcp = app.mcp().await;
    let sum: AddOut = mcp
        .call_tool("add", serde_json::json!({ "a": 2.0, "b": 3.0 }))
        .await
        .unwrap();
    assert_eq!(sum.value, 5.0);

    let admin = app.mcp().as_user("alice", &["admin"]).await;
    assert!(admin.tool_names().await.contains(&"add".to_string()));
}
```

The client also offers `list_tools`, `call_tool_text`, `call_tool_raw`,
`read_resource`, `get_prompt` and `wait_for` (notifications).

## See also

- Full guide: [docs/features/25-mcp.md](https://github.com/plawn/r2e/blob/master/docs/features/25-mcp.md)
- [Guards and Roles](../security/guards-and-roles.md)
