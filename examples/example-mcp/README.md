# example-mcp

An MCP server (`r2e-mcp`) and an HTTP API sharing the same beans.

What it shows:

- `MathTools`: `#[tool]` members `add`, `divide`, `call_log`, `clear_log`
  (guarded by an `ApiKeyGuard`), `enable_scientific`, and `remember`, which
  registers a dynamic tool at runtime
- A `#[resource]` (`r2e://calc/call-log`) and a `#[prompt]`
  (`explain_division`)
- `ScientificTools` (`sqrt`) as an opt-in group:
  `#[routes(group = "scientific", opt_in)]`, enabled by `enable_scientific`
- `CalcController`: the same calculator over HTTP,
  `GET /api/calc/add/{a}/{b}`
- One type = one port: MCP members and HTTP routes live on separate types

## Running

```bash
cargo run -p example-mcp
```

The MCP endpoint is `http://localhost:3000/mcp` (streamable HTTP). Connect with
the MCP Inspector:

```bash
npx @modelcontextprotocol/inspector
# transport: Streamable HTTP, URL: http://localhost:3000/mcp
```

`clear_log` requires the header `x-api-key: letmein`. The path and the worker
count come from config, so `R2E_MCP_PATH=/tools` or `R2E_SERVER_WORKERS=4`
override them.

## Testing

```bash
cargo test -p example-mcp
```

`tests/mcp_e2e.rs` drives the server end to end.
