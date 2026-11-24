# r2e-mcp

MCP (Model Context Protocol) server support for R2E — expose tools, resources and prompts to agents with the same DX as HTTP controllers.

## Overview

`#[tool]`, `#[resource]` and `#[prompt]` methods on a `#[routes]` impl are served over MCP's streamable HTTP transport. They get the full R2E runtime: `#[inject]` / `#[config]` fields, the **same** guards (`#[roles]`, `#[guard]`) and interceptors as HTTP, typed config, graceful shutdown and sharded serving. A missing bean is a compile error at `register_mcp_service`.

One type exposes one port: HTTP routes and MCP members live on separate types over the same beans (an MCP member next to an HTTP route in one impl is a compile error).

rmcp is the wire only — R2E dispatches members itself, and every rmcp type users need is re-exported here, so application code never imports rmcp.

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.15", features = ["mcp"] }
schemars = "1"
```

### Declaring tools

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

#[derive(serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct AddOut {
    pub value: f64,
}

#[controller]
pub struct MathTools {
    #[inject] calc: CalcService,
}

#[routes]
impl MathTools {
    /// Add two numbers.            // doc comment → tool description
    #[tool(read_only, idempotent)]
    async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut> {
        Json(AddOut { value: self.calc.add(p.a, p.b) })
    }
}
```

The `schemars` schema of `AddIn` becomes the tool's `inputSchema`; `Json<T>` returns are sent as `structuredContent` + text and advertise an `outputSchema`. Domain failures return `McpError::tool(...)` (an `isError: true` result the agent can read).

### Mounting the endpoint

```rust
AppBuilder::new()
    .plugin(McpServer::new())               // mounts the endpoint at /mcp
    .provide(CalcService::default())
    .build_state().await
    .register_mcp_service::<MathTools>()    // compile-time bean check
    .serve_auto().await
```

Point the MCP Inspector at `http://localhost:3000/mcp`.

## Configuration

All keys are optional; `McpServer` builder methods take precedence.

```yaml
mcp:
  path: /mcp
  name: my-server                  # serverInfo.name
  stateless: false                 # true → no MCP sessions (load balancers without sticky sessions)
  allowed-hosts: [api.example.com] # Host allowlist; loopback only by default — set it behind a proxy
  session:
    idle-timeout-secs: 300
  auth:
    issuer: https://idp.example.com/realms/app   # OAuth 2.1 resource server (with server.public-url)
```

## Key types

| Type | Description |
|------|-------------|
| `McpServer` | `Plugin` — mounts the streamable-HTTP endpoint (config `mcp.*`) |
| `Params<T>` / `ObjectParams` | Typed member arguments; `T: Deserialize + JsonSchema + ObjectParams` |
| `ToolCall` | Raw call context: arguments, transport request parts, request id, cancel token |
| `McpError` | Error type for members (`McpError::tool(...)` → `isError` result) |
| `McpSession` | The caller's session handle (per-session dynamic members) |
| `Progress` | `notifications/progress` reporter for long calls |
| `McpClient` | Elicitation — ask the user for input mid-call |
| `IntoToolResult` / `IntoResourceResult` / `IntoPromptResult` | Return-type conversions for members |
| `McpConfig` | Typed `mcp.*` config section |

## Features

| Feature | Facade feature | Description |
|---------|----------------|-------------|
| `loopback` | `mcp-loopback` | In-process MCP sessions on the mounted endpoint (`r2e_mcp::loopback`) |
| `testing` | `mcp-testing` | Test helpers (`app.mcp()`, `pin_mcp_validator`); dev-dependencies only |

## Testing

Enable `mcp-testing` from `[dev-dependencies]` (`r2e = { version = "0.15", features = ["mcp-testing"] }`), then:

```rust
use r2e::r2e_mcp::testing::McpTestExt;
use r2e_test::TestApp;

#[r2e::test(app = McpApp)]
async fn adds(app: TestApp) {
    let mcp = app.mcp().await;
    let sum: AddOut = mcp.call_tool("add", serde_json::json!({ "a": 2.0, "b": 3.0 })).await.unwrap();
    assert_eq!(sum.value, 5.0);
}
```

## Documentation

Full guide (resources, prompts, completion, elicitation, dynamic members, auth): [`docs/features/25-mcp.md`](../docs/features/25-mcp.md).

## License

Apache-2.0
