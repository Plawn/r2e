---
topic: mcp-server
features: mcp
tokens: ~4900
requires: guards, security
---

## MCP Server (Model Context Protocol)

### TL;DR

- Enable feature `mcp`; declare the service with `#[controller]` +
  `#[mcp_routes]` and register it with `.register_mcp_service::<T>()` after
  `build_state()` (deps and config keys are checked there).
- Install the `McpServer::new()` plugin (endpoint `/mcp` by default);
  everything else is `mcp.*` config, which builder methods override.
- One marker per method: `#[tool]`, `#[resource(uri = …)]` (fixed URI, no
  `Params<T>`), `#[prompt]`. Doc comments become the descriptions the agent
  reads.
- A member takes `&self` plus at most one of `Params<T>` (typed arguments →
  `inputSchema`, needs `Deserialize + JsonSchema + ObjectParams`),
  `ToolCall`/`ResourceCall`/
  `PromptCall`, `CancelToken`. Beans and config go on the struct, never as
  parameters.
- Return `Json<T>` for structured output (`structuredContent` + `outputSchema`),
  `String`/`&str`/`()`, `CallToolResult`, or `Result<_, E: Into<McpError>>`;
  `McpError::tool(...)` is an agent-readable `isError` result, other variants
  become JSON-RPC errors.
- Guards, interceptors and `#[roles]` are the same HTTP machinery and work on
  all three families (see llm/guards.md); per-member scopes are
  `#[tool(scopes = "…")]` (ALL) or `any_scopes` (at least one).
- Read the caller with `#[inject(identity)]` on a member parameter (or
  `Option<…>`), which compiles to `ToolCall::identity::<T>()`.
- A duplicate tool name, fixed resource URI, URI template or prompt name across
  services is a **boot panic**; one type cannot be both `#[routes]` and
  `#[mcp_routes]`.
- Per-session lists: `#[mcp_routes(group = "git", opt_in)]` hides a service
  until a session enables it; a member parameter `session: McpSession` lets a
  tool `enable_group`/`add_tool(DynamicTool::…)`; `McpServer::session_init::<T>()`
  shapes the list from the opening request; the `McpSessions` bean reaches
  live sessions by subject. Hidden = unknown (not callable).
- Auth: set `server.public-url` + `mcp.auth.issuer` (any OIDC IdP) and the rest
  is discovered; with no `mcp.auth` section the server is unauthenticated.
- Set `mcp.allowed-hosts` behind a proxy or public hostname (the default
  accepts loopback `Host` headers only); tests boot with `pin_mcp_validator`
  **before** `load_config` (feature `mcp-testing`).

Requires feature: `mcp`. Exposes tools, resources and prompts to MCP clients (Claude, MCP Inspector,
agents) over MCP's streamable-HTTP transport, with the same DX as HTTP
controllers: `#[inject]`, `#[config]`, guards (the SAME `Guard<I>` machinery
HTTP uses — `#[guard(...)]` works on tools), interceptors, typed config,
graceful shutdown, sharded serving. rmcp is the wire only — R2E dispatches
tools itself; no rmcp type appears in user code.

```rust
use r2e::prelude::*;                       // Params, ToolCall, McpError, McpServer, AppBuilderMcpExt (feature `mcp`)
use schemars::JsonSchema;

#[derive(serde::Deserialize, JsonSchema, ObjectParams)]   // ObjectParams: named-struct marker Params<T> requires
pub struct AddIn {
    /// Left operand.                      // doc comments → JSON-Schema property descriptions
    pub a: f64,
    pub b: f64,
}

#[derive(serde::Serialize, JsonSchema)]
pub struct AddOut { pub value: f64 }

#[controller]                              // same attribute as HTTP: #[inject]/#[config] fields
pub struct MathTools {
    #[inject] calc: CalcService,
}

#[mcp_routes]
#[intercept(Logged::info())]               // impl-level interceptor wraps every tool
impl MathTools {
    /// Add two numbers.                   // doc comment → tool description shown to the agent
    #[tool(read_only, idempotent)]         // annotations: read_only/destructive/idempotent/open_world/name/title
    async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut> {
        Json(AddOut { value: self.calc.add(p.a, p.b) })
    }

    /// Divide `a` by `b`.
    #[tool(name = "divide")]               // wire name override
    async fn div(&self, Params(p): Params<AddIn>) -> Result<Json<AddOut>, McpError> {
        self.calc.divide(p.a, p.b).map(|value| Json(AddOut { value }))
            .ok_or_else(|| McpError::tool("division by zero"))   // agent-readable isError result
    }

    /// Clear state. Requires the `x-api-key` header.
    #[tool(destructive)]
    #[guard(ApiKeyGuard)]                  // real Guard<I> — reads transport request headers
    async fn clear(&self, call: ToolCall) -> String {            // ToolCall: raw args, HTTP parts, request id, CancelToken
        format!("cleared (request {})", call.request_id)
    }
}
# fn main() {}
```

```rust
use r2e::prelude::*;                       // McpServer + AppBuilderMcpExt are in the prelude (feature `mcp`)

# async fn __doc(b: AppBuilder) -> impl Sized {
# let b = b.register::<CalcService>();   // the bean MathTools injects
b.plugin(McpServer::new()                  // endpoint at `/mcp` by default
        .with_name("my-server")            // advertised serverInfo (defaults: CARGO_PKG_NAME/VERSION)
        .with_instructions("..."))
 .build_state().await
 .register_mcp_service::<MathTools>()      // deps + config keys checked at compile time here
# }
```

Tool signatures — `&self` plus at most one of each: `Params<T>` (typed
arguments; `T: Deserialize + JsonSchema + ObjectParams` becomes the
`inputSchema`), `ToolCall`
(raw arguments, HTTP request parts, JSON-RPC id, cancellation token),
`CancelToken`. Returns: `String`/`&str`/`()`, `Json<T: Serialize + JsonSchema>`
(dual-encoded: `structuredContent` + JSON text, advertises `outputSchema`),
`CallToolResult`, or `Result<_, E: Into<McpError>>`. `McpError::tool(...)` →
`isError: true` result the agent can read; other variants map to JSON-RPC
errors (bad args → -32602, unknown tool → -32601, guard rejection → -32600 with
the HTTP status text). Beans/config go on the struct, never as parameters.

Resources and prompts — the same impl block can carry the two other MCP
member families (one marker per method):

```rust
# #[controller]
# pub struct MathTools { #[inject] log: CallLog }
#[mcp_routes]
impl MathTools {
    /// The call log, one entry per line.  // doc comment → description
    #[resource(uri = "r2e://calc/call-log", mime_type = "text/plain")]   // uri REQUIRED, fixed (no templates)
    async fn call_log(&self) -> String { self.log.entries().join("\n") }

    /// Walk the agent through a division.
    #[prompt(name = "explain_division")]   // Params<T> schema → advertised prompt arguments
    async fn explain(&self, Params(p): Params<AddIn>) -> String {
        format!("Divide {} by {} using the `divide` tool.", p.a, p.b)
    }
}
# fn main() {}
```

`#[resource(uri, name, title, description, mime_type, scopes, any_scopes)]`:
no `Params<T>` (`resources/read` carries no arguments); `ResourceCall` instead
of `ToolCall`; returns `String`/`&str` (text with the declared MIME type),
`Json<T>` (`application/json`), `ResourceContents`/`Vec<ResourceContents>`, or
`Result<_, E: Into<McpError>>`. `#[prompt(name, title, description, scopes,
any_scopes)]`: `Params<T>` drives deserialization AND the advertised
`arguments` (requiredness + doc-comment descriptions; MCP argument values are
strings — prefer string-typed fields); `PromptCall` instead of `ToolCall`;
returns `String`/`&str` (one `user` message), `PromptMessage`/`Vec<PromptMessage>`,
`GetPromptResult`, or `Result`. Resource URIs and prompt names are global
across services (duplicate = boot panic, like tool names). Capabilities are
advertised only for non-empty families. Resources/prompts have NO in-result
error plane: `McpError::tool(...)` degrades to JSON-RPC -32603 (message
preserved); unknown resource URI → -32002, unknown prompt name → -32602.
Guards/interceptors/scopes/roles work identically on all three families, and
`resources/list`, `resources/templates/list` and `prompts/list` are filtered by
caller access like `tools/list`.

One type cannot be both `#[routes]` and `#[mcp_routes]` — share a bean between
a thin HTTP controller and a thin MCP service instead (see
`examples/example-mcp`).

Config under `mcp.*` (all optional; builder methods override config):
`path` (default `/mcp`), `enabled` (true), `name`/`version`/`instructions`,
`sse-keep-alive-secs` (15; 0 disables), `stateless` (false),
`json-response` (false; stateless only), `allowed-hosts` (DNS-rebinding
protection — REQUIRED behind a proxy/public hostname; default accepts loopback
`Host` headers only), `allowed-origins`, `max-request-body-bytes`.

Sharded serving (`server.workers`) shares one session map across workers;
graceful shutdown terminates live SSE streams. Test over `TestApp`: POST the
JSON-RPC envelope to `/mcp` with `accept: application/json, text/event-stream`
(responses are SSE `data:` events; see `examples/example-mcp/tests/mcp_e2e.rs`).

### Dynamic members — per-session lists

Every session starts from the boot-time catalog; groups, a session handle and
session-private members change what ONE session sees. Typed `#[mcp_routes]`
members stay compile-checked; the dynamism is *which* members a session sees.

```rust
#[derive(serde::Deserialize, schemars::JsonSchema, ObjectParams)]
pub struct TableIn { pub table: String }

#[derive(serde::Deserialize, schemars::JsonSchema, ObjectParams)]
pub struct QueryIn { pub sql: String }

#[controller]
pub struct GitTools;

#[mcp_routes(group = "git", opt_in)]      // whole service in group "git", hidden until enabled
impl GitTools {
    /// Working tree status.
    #[tool]
    async fn git_status(&self) -> &'static str { "clean" }
}

#[controller]
pub struct Toolbox;

#[mcp_routes]
impl Toolbox {
    /// Reveal the git tools to this session.
    #[tool]
    async fn enable_git(&self, session: McpSession) -> Result<&'static str, McpError> {
        session.enable_group("git")?;      // + notifications/tools/list_changed
        Ok("git tools enabled")
    }

    /// Create a session-private query tool for one table.
    #[tool]
    async fn open_table(&self, Params(p): Params<TableIn>, session: McpSession) -> Result<String, McpError> {
        let name = format!("query_{}", p.table);
        session.add_tool(
            DynamicTool::new(name.clone())
                .description("Run a read-only query")
                .scopes(&["db:read"])                     // same scope/role gates as #[tool]
                .handler(|q: QueryIn, _call: ToolCall| async move { format!("ran {}", q.sql) }),
        )?;                                               // name collision → Err, never a panic
        Ok(name)
    }

    /// Per-member group, visible by default (no `opt_in`).
    #[tool(group = "admin")]
    async fn stats(&self) -> &'static str { "42" }
}

#[derive(Clone)]
pub struct RoleToolsets;

impl McpSessionInit for RoleToolsets {
    async fn init(&self, s: &SessionInit) -> Result<SessionToolset, McpError> {
        let toolset = SessionToolset::new().enable_if(s.has_role("dev"), "git");
        // "admin" is on by default (no `opt_in`): switch it off for non-admins.
        Ok(if s.has_role("admin") { toolset } else { toolset.disable("admin") })
    }
}

# async fn __doc(b: AppBuilder) -> impl Sized {
b.provide(RoleToolsets)                                    // the hook is a bean
    .plugin(McpServer::new().session_init::<RoleToolsets>())
    .build_state().await
    .register_mcp_service::<GitTools>()
    .register_mcp_service::<Toolbox>()
# }
# fn main() {}
```

- Groups: `#[mcp_routes(group = "…")]` (service) or `#[tool|resource|prompt(group
  = "…")]` (member, overrides); `opt_in` = hidden until enabled (needs a
  `group` — compile error otherwise). No group = always visible (existing apps
  unchanged).
- A hidden or foreign member answers **exactly like an unknown one** (tool
  -32601, resource -32002, prompt -32602); scope/role filtering still applies
  on top, to private members too.
- `McpSession` (at most one per member; also `ToolCall::session`,
  `ResourceCall::session`, `PromptCall::session`): `enable_group`,
  `disable_group`, `add_tool`/`remove_tool`, `add_resource`/`remove_resource`
  (`DynamicResource::new(uri, name).handler(|call: ResourceCall| async { … })`),
  `add_prompt`/`remove_prompt` (`DynamicPrompt::new(name).handler(|p: T, call:
  PromptCall| …)`), `apply(SessionToolset)` (batched, one notification),
  `is_group_enabled`, `has_tool`, `subject`. A private member cannot shadow a
  catalog name, even a hidden one.
- `McpSessionInit::init` runs once per session, lazily on its first request
  (`SessionInit`: `principal`, `subject`, `has_role`, `has_scope`,
  `identity::<T>()`, `header`, `parts`); `Err` fails that request and is
  retried. A missing hook bean is a boot panic.
- `McpSessions` (plugin-provided bean — `#[inject] sessions: McpSessions`):
  `for_subject(sub)`, `all()`, `len()`; each item is an `McpSession`. Use it
  from an admin route or a `#[consumer]` to change a live user's toolset.
- Sessions are **bound to the principal that opened them**: under `mcp.auth`,
  the same session id with another token's subject gets HTTP `404` (identical
  to an unknown session) on `POST`, SSE `GET` and `DELETE`.
- Once lists are dynamic (any `opt_in` group, a `session_init`, or a member
  taking `McpSession`), all three families advertise `listChanged: true`.
- `mcp.stateless: true`: groups and `session_init` work (recomputed per
  request); a member taking `McpSession` is a **boot panic** (mutations would
  not outlive the request).

### MCP auth — OAuth 2.1 resource server (`mcp.auth.*`)

Works with ANY OIDC IdP (Keycloak, Auth0, Entra, Okta). Three YAML keys:

```yaml
server:
  public-url: https://api.example.com     # the app's external origin (framework-wide key)
mcp:
  auth:
    issuer: https://id.example.com/realms/acme
    public-client-id: mcp-public          # optional; enables the static DCR shim
```

Everything else is discovered from the issuer. You get: local JWT validation
(JWKS, zero network per request), audience = canonical resource URI
(`mcp.auth.resource`, default `{server.public-url}{mcp.path}`), 401 challenges
with `WWW-Authenticate: Bearer resource_metadata="…"` (RFC 9728), public
protected-resource metadata at `/.well-known/oauth-protected-resource{path}`,
and — when `public-client-id` is set — a DCR shim (`POST {path}/oauth/register`
hands every client that pre-created public client id; redirect URIs must
already be configured on the IdP client). IdP outages → 503, not 401. No
`mcp.auth` section → unauthenticated server.

Other `mcp.auth.*` keys (all optional): `resource`/`resource-name`, `discovery`
(`eager`|`lazy`|`off`) + `discovery-ttl-secs`/`jwks-url`/explicit endpoints,
`token-validation` (`jwt` default — local JWKS, zero network per request;
`introspection` — RFC 7662 for opaque tokens, requires the confidential
`client-id` + `client-secret`; `userinfo` — OIDC userinfo probe for
Google-style opaque tokens, forces `audience: skip`) with
`introspection-endpoint`/`userinfo-endpoint` overrides and an opaque-token
cache (`opaque-cache-ttl-secs` 60 — capped by the token's `exp`, rejections
cached 5s, IdP outages never; `opaque-cache-max-entries` 1024),
`allowed-algorithms`, `clock-skew-secs`, `audience`
(`resource`|`any-of`|`client-id`|`skip`) + `extra-audiences` (Auth0/Entra),
`required-scopes` (server-wide floor → 403), `scopes-supported`, `scope-claim`
(default `scope` then `scp`; Auth0 RBAC → `permissions`), `roles-claim`
(default `roles` + Keycloak `realm_access.roles`), `client-roles-for`
(Keycloak `resource_access.<id>.roles`), `shim`, `registration-path`,
`redirect-uri-allowlist`, `extra-authorize-params` (map merged into every
authorization request — Auth0 `audience`, Google `access_type=offline`;
requires the shim: the mirrored metadata's `authorization_endpoint` is
rewritten to `{mcp.path}/oauth/authorize`, which 302-redirects to the IdP
with the params applied, server config winning over client duplicates),
`filter-members` (true — list operations hide members the
caller cannot invoke), `allow-insecure` (http issuer, dev), `allowed-origins`.

Per-tool authorization:

```rust,ignore
#[tool(scopes = "mcp:read")]                       // caller must hold ALL listed scopes
#[tool(any_scopes = ["mcp:admin", "mcp:write"])]   // at least ONE
#[roles("admin")]                                  // same RolesGuard as HTTP routes
async fn secure(&self, #[inject(identity)] user: AuthenticatedUser) -> String { … }
```

Scope denials are JSON-RPC errors with agent-actionable text ("re-authorize
requesting them"). `#[inject(identity)]` on a member parameter (or `Option<…>`)
reads the validated principal — on all three families, not just tools.

`McpPrincipal.user` is an `Arc<AuthenticatedUser>`, and the auth layer deposits
that same handle as the identity extension, so no request holds two copies of
its caller: `principal.user.sub` still works through `Deref`, an owned copy is
`(*principal.user).clone()`. What `#[inject(identity)]` compiles to is
`ToolCall::identity::<T>()` (also `ResourceCall::identity` /
`PromptCall::identity`) — `Option<T>`, resolved from the transport request
extensions:

```rust
# fn __doc(call: ToolCall) {
call.identity::<AuthenticatedUser>();       // Arc<T> extension first  -> (*arc).clone(), the ONLY copy,
                                            //   made only for a member that declares an identity
                                            // plain T extension second -> .clone() (any other layer
                                            //   that inserts an identity by value)
                                            // neither, or no HTTP parts -> None
call.identity::<Arc<AuthenticatedUser>>();  // hits the plain-T arm, finds the shared handle: no copy
# }
```

`Arc<T>` wins when both are present. `extension::<T>()` is unchanged — a plain
`get::<T>().cloned()`, with no `Arc` step.

`ToolRoute::description` is `Option<Cow<'static, str>>` (rmcp's `Tool` stores a
`Cow`, and `tools/list` clones the prebuilt list per request, so `#[tool]`
emits `Cow::Borrowed` and the clone copies a pointer pair). Hand-built routes:
`Some(s.into())` for a `String`, `Some("…".into())` for a literal.

Testing (feature `mcp-testing` on the `r2e` facade): boot with zero network I/O —

```rust
use r2e::r2e_mcp::testing::pin_mcp_validator;
# fn __doc() {
let jwt = TestJwt::for_resource("http://localhost:3000/mcp");   // aud = resource
let b = pin_mcp_validator(AppBuilder::new(), &jwt, "http://localhost:3000/mcp"); // BEFORE load_config
let token = jwt.token_builder("alice").scopes(&["mcp:write"]).build();
// TokenBuilder also: .audiences(&[...]) (array aud), .realm_roles/.client_roles (Keycloak),
// .claim("scp", ...) (Entra/Okta), .expired()
# }
```

See the authenticated smoke in `examples/example-mcp/tests/mcp_e2e.rs`;
exhaustive protocol and auth cases live in `r2e-mcp/tests`.

### Do not

- Do not take beans or config as member parameters — they are `#[inject]` /
  `#[config]` fields on the `#[controller]` struct.
- Do not expect an in-result error plane on resources and prompts:
  `McpError::tool(...)` degrades there to JSON-RPC -32603.
- Do not give a prompt non-string argument fields — MCP argument values are
  strings.
- Do not try to hide a tool by filtering `tools/list` yourself — use a group;
  an `opt_in` member is also uncallable, a filtered one would not be.
- Do not make one type both `#[routes]` and `#[mcp_routes]`; share a bean
  between a thin HTTP controller and a thin MCP service instead.
