# MCP Server (Model Context Protocol)

**Crate:** `r2e-mcp` · **Feature flag:** `mcp` (in `full`)

Expose tools to MCP clients (Claude, `npx @modelcontextprotocol/inspector`,
any agent) over MCP's **streamable-HTTP** transport, with the full R2E
runtime behind them: `#[inject]` beans, `#[config]`, guards, interceptors,
typed config, graceful shutdown, sharded serving, `TestApp`.

rmcp is used for the wire protocol only — **R2E dispatches tools itself**.
User code never imports rmcp directly; the raw wire escape hatches
(`CallToolResult`, `GetPromptResult`, `PromptMessage`, `PromptMessageRole`,
`ResourceContents`) are deliberately re-exported by `r2e-mcp` for hand-built
results.

## TL;DR

Expose MCP tools to agents (Claude, MCP Inspector) with the full R2E runtime behind them. `#[controller]` + `#[mcp_routes]` on a dedicated type turn plain async methods into tools: `#[tool(read_only)] async fn add(&self, Params(p): Params<AddIn>) -> Json<AddOut>` — the `schemars` schema of `AddIn` becomes the `inputSchema` (doc comments → descriptions), `Json<T>` returns are dual-encoded (`structuredContent` + text) and advertise an `outputSchema`. Guards and interceptors are the SAME machinery as HTTP (`#[guard(...)]` reads real transport headers; `#[intercept(...)]` specs are prebuilt from the bean graph), and a missing bean is a compile error at `.register_mcp_service::<T>()`. Setup is one line: `.plugin(McpServer::new())` mounts the streamable-HTTP endpoint at `/mcp` (config `mcp.*`), shares one session map across SO_REUSEPORT workers, and terminates live SSE streams on graceful shutdown. rmcp is the wire only — raw wire escape hatches are re-exported by R2E, so user code never imports rmcp directly. Requires feature `mcp`. Domain failures return `McpError::tool(...)` → `isError: true` results the agent can read; set `mcp.allowed-hosts` behind a proxy (loopback-only `Host` allowlist by default). Auth: two YAML keys (`mcp.auth.issuer` + `server.public-url`) make the endpoint an OAuth 2.1 resource server for any OIDC IdP — JWKS-backed JWT validation, RFC 9728 protected-resource metadata + `WWW-Authenticate` challenges, and (with `public-client-id`) a static DCR shim for IdPs without dynamic client registration (Keycloak, Google, Entra, Okta); per-tool `#[tool(scopes = ...)]` + shared `#[roles]` guards, `tools/list` filtered to what the caller can invoke, and a no-Docker test fast path (`TestJwt::for_resource` + `pin_mcp_validator`, feature `mcp-testing`).

## Quick start

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
| `Params<T>` | Typed tool arguments. `T: Deserialize + JsonSchema + ObjectParams`; derive all three on a named-field struct. The sealed marker rejects scalar/tuple/enum root schemas at compile time. The schema becomes the tool's `inputSchema` (doc comments → property descriptions, `Option<..>` fields → not required, nested types/enums kept as inline `$defs`). |
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

## Resources and prompts

The same impl block can expose the two other MCP member families —
`#[resource]` (fixed or templated readable data) and `#[prompt]` (reusable message
templates). One method = one member; a method carries exactly one of the
three markers. All members of a service share ONE core (built once from the
bean graph) and the same prebuilt guard/interceptor sets.

```rust
#[mcp_routes]
impl MathTools {
    /// The call log, one entry per line.
    #[resource(uri = "r2e://calc/call-log", mime_type = "text/plain")]
    async fn call_log(&self) -> String { self.log.entries().join("\n") }

    #[resource(uri = "r2e://users/{id}", mime_type = "application/json")]
    async fn user(&self, call: ResourceCall) -> Json<User> {
        Json(self.users.get(&call.variables["id"]).await?)
    }

    /// Walk the agent through a division.
    #[prompt(name = "explain_division")]
    async fn explain(&self, Params(p): Params<BinaryOperands>) -> String {
        format!("Divide {} by {} using the `divide` tool.", p.a, p.b)
    }
}
```

- `#[resource(uri = "…")]` — the URI is **required**. A value containing an
  RFC 6570 expression such as `{id}` is advertised through
  `resources/templates/list`; other values remain in `resources/list`.
  Templates and fixed URIs are global across services (a duplicate is a boot
  panic naming both). Optional keys: `name` (defaults to
  the method name), `title`, `description`, `mime_type` (advertised in
  `resources/list` and applied to text-shaped returns), `scopes`/`any_scopes`.
  Resource methods take **no `Params<T>`** (`resources/read` carries no
  arguments); `ResourceCall` replaces `ToolCall` (it adds the requested
  `uri` plus the captured template `variables`). Return types
  (`IntoResourceResult`): `String`/`&str` (text contents
  with the declared MIME type), `Json<T>` (`application/json` unless
  overridden), `ResourceContents` / `Vec<ResourceContents>`, and `Result<_,
  E: Into<McpError>>`.
- `#[prompt]` — optional `name` (unique across services), `title`,
  `description`, `scopes`/`any_scopes`. A `Params<T>` parameter drives BOTH
  deserialization and the advertised `arguments` list (one entry per schema
  property: requiredness from `required`, descriptions from field doc
  comments — per the MCP spec argument *values* arrive as strings, so prefer
  string-typed fields for maximum client compatibility). `PromptCall`
  replaces `ToolCall`. Return types (`IntoPromptResult`): `String`/`&str`
  (one `user` message), `PromptMessage` / `Vec<PromptMessage>`,
  `GetPromptResult`, and `Result<_, E: Into<McpError>>`; the doc-comment
  description is attached to the result unless it already carries one.

`resources` / `prompts` capabilities are only advertised when at least one
member of that family exists. **Error mapping differs from tools**: resources
and prompts have no in-result error plane, so `McpError::Tool` degrades to a
JSON-RPC `-32603` internal error (message preserved, structured data
attached); everything else maps as in the table above. An unknown resource
URI is `-32002` (`RESOURCE_NOT_FOUND`); an unknown prompt name is `-32602`
(`invalid_params`, per the MCP spec).

Resource subscriptions are enabled whenever resources exist. Inject the
plugin-provided `McpResourceUpdates` bean and call `updates.notify(uri)` after
changing a resource. R2E sends `notifications/resources/updated` only to
sessions subscribed to that exact URI. Both legacy `resources/subscribe` /
`resources/unsubscribe` and the current `subscriptions/listen` filter are
handled; lagging sessions skip stale update hints (clients re-read the
resource for the actual contents).

Auth is uniform across families: `scopes`/`any_scopes` on the marker,
`#[roles]`/`#[all_roles]`/`#[guard]` via the shared guard machinery, and
`resources/list` / `prompts/list` filtered to what the caller may access
(same `mcp.auth.filter-members` switch); denials are JSON-RPC `-32600` errors
with `data: "forbidden"` instead of `isError` results.

## Guards and interceptors

All members (tools, resources, prompts) use the **same** `Guard<I>` /
`Interceptor<R>` machinery as HTTP routes
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

Impl-level decorators are built once per MCP service and shared by all of its
members. In particular, a stateful impl-level guard/interceptor keeps one state
across tools, resources and prompts; an impl-level guard receives
`method_name: "*"`, matching the controller-wide HTTP semantics. Method-level
decorators remain one built product per member. The generated service uses one
Arc containing the core and all decorator products, with no nested Arc or
per-member decorator container.

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
  allowed-origins: []         # reject browser requests from other Origins
  cors:
    allowed-origins: [https://claude.ai] # origins granted browser CORS access
  max-request-body-bytes: 4194304
```

The two origin lists have different jobs. `mcp.allowed-origins` is the
transport-level request rejection list (empty disables that check), while
`mcp.cors.allowed-origins` controls the CORS response headers. CORS defaults
to `https://claude.ai` and `https://claude.com`, plus localhost origins under
the `dev` profile.

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

## Auth — OAuth 2.1 resource server (`mcp.auth.*`)

Two YAML keys turn the endpoint into a spec-compliant OAuth 2.1 resource
server for **any** OIDC IdP:

```yaml
server:
  public-url: https://api.example.com      # the app's external origin
mcp:
  auth:
    issuer: https://id.example.com/realms/acme
    public-client-id: mcp-public           # optional; presence enables the DCR shim
```

Everything else derives from `{issuer}/.well-known/openid-configuration`:
JWKS URL, endpoints, algorithms. No `mcp.auth` section ⇒ unauthenticated
server (local dev). Remove the `mcp.auth` section to disable authentication.

What you get, with zero additional code:

- **Bearer validation** on every MCP request: local JWT verification against
  the IdP's JWKS (zero network per request), audience bound to the canonical
  resource URI (RFC 8707), issuer + expiry + algorithm checks.
- **401 challenges** with `WWW-Authenticate: Bearer resource_metadata="…"`
  (RFC 9728) — MCP clients (Claude, Inspector) bootstrap the whole OAuth
  flow from this header.
- **Protected-resource metadata** at
  `/.well-known/oauth-protected-resource[{mcp.path}]` (public, CORS-open,
  cached).
- **Static DCR shim** (when `public-client-id` is set): the server mirrors
  the IdP's authorization-server metadata and serves a
  `POST {mcp.path}/oauth/register` endpoint that hands every client the same
  pre-created public client id. This is what makes Keycloak (anonymous DCR
  blocked), Google, Entra and Okta (no DCR) work with clients that expect
  RFC 7591. **The shim registers nothing** — redirect URIs must already be
  configured on the IdP client; requested ones are filtered against
  `redirect-uri-allowlist` (default: localhost any-port, the Claude
  callbacks, the MCP Inspector).
- **Authorize-redirect shim** (when `extra-authorize-params` is set): the
  mirrored metadata's `authorization_endpoint` points at
  `GET {mcp.path}/oauth/authorize`, which merges the configured params into
  the client's query (server config wins over client-sent duplicates) and
  302-redirects to the IdP's real endpoint. This is how Auth0 gets its
  `audience=` and Google its `access_type=offline` from clients that don't
  know to send them.
- **IdP outages are 503, not 401** — clients aren't sent into a re-auth loop
  when JWKS/discovery are briefly unreachable (stale-if-error caches on
  both).

### The canonical resource URI

The token audience, the PRM `resource` and the challenge URL all use one
canonical URI, resolved in order:

1. `mcp.auth.resource` (explicit),
2. `{server.public-url}{mcp.path}` (recommended),
3. dev/test fallback `http://{host}:{port}{mcp.path}` — only under the
   `dev`/`test` profile or a loopback bind, with a boot `warn!`,
4. otherwise a boot error naming the keys to set.

It is canonicalized once (lowercased scheme/host, default port dropped, no
trailing slash/query/fragment) and never derived from `Host` /
`X-Forwarded-*` headers (attacker-controlled).

### Per-tool authorization

```rust
#[mcp_routes]
impl AdminTools {
    #[tool(scopes = "mcp:read")]                      // must hold ALL listed scopes
    async fn read_data(&self) -> &'static str { "data" }

    #[tool(any_scopes = ["mcp:admin", "mcp:write"])]  // at least ONE
    async fn flexible(&self) -> &'static str { "ok" }

    #[tool]
    #[roles("admin")]                                 // same guard as HTTP routes
    async fn admin_only(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
        format!("admin:{}", user.sub)
    }
}
```

- Server-wide `mcp.auth.required-scopes` → HTTP 403 + `insufficient_scope`
  challenge naming the missing scopes.
- `#[tool(scopes/any_scopes)]` → JSON-RPC-level denial with agent-actionable
  text ("re-authorize requesting them"), checked before identity/guards.
- Declaring `scopes`/`any_scopes` while `mcp.auth` is absent or disabled is a
  boot error; a scoped member can never silently become public.
- `#[roles]` / `#[all_roles]` → the shared `RolesGuard` over the validated
  principal's roles — identical semantics to HTTP routes.
- `tools/list` is filtered by the caller's scopes/roles by default
  (`mcp.auth.filter-members: false` lists everything; invocation checks still
  apply).
- `#[inject(identity)] user: AuthenticatedUser` (or `Option<…>`) on a tool
  parameter reads the authenticated principal; a required identity with no
  authenticated caller is a JSON-RPC `unauthorized` error.

### Configuration reference

| key | default | notes |
|---|---|---|
| `issuer` | — (required) | must be `https://` unless `allow-insecure: true` |
| `resource`, `resource-name` | derived | canonical resource URI + display name |
| `discovery` | `eager` (`lazy` under `dev` profile) | `eager` = fetch at boot, fail fast; `lazy` = first use; `off` = explicit endpoints only (requires `jwks-url` / `introspection-endpoint` / `userinfo-endpoint`, per validation mode) |
| `discovery-ttl-secs` | 3600 | metadata cache TTL (stale-if-error) |
| `jwks-url`, `authorization-endpoint`, `token-endpoint`, … | discovered | explicit overrides |
| `token-validation` | `jwt` | `jwt` = local JWKS validation (zero network per request); `introspection` = RFC 7662 (opaque tokens; requires `client-id` + `client-secret`); `userinfo` = OIDC userinfo probe (Google-style opaque tokens; forces `audience: skip`) |
| `client-id`, `client-secret` | — | confidential client the introspection backend authenticates as (Basic) |
| `introspection-endpoint`, `userinfo-endpoint` | discovered | explicit overrides for the opaque backends |
| `opaque-cache-ttl-secs` | 60 | positive validation cache for opaque tokens (capped by the token's `exp`; rejections cached 5s, outages never; concurrent misses for one token share one IdP request) |
| `opaque-cache-max-entries` | 1024 | opaque-token cache size cap; least-recently-used live entry evicted at capacity |
| `allowed-algorithms` | RS256, ES256, PS256 | JWT signature algorithms |
| `clock-skew-secs` | 60 | leeway for `exp`/`nbf` |
| `audience` | `resource` | `any-of` (+ `extra-audiences`), `client-id`, `skip` |
| `required-scopes` | — | server-wide scope floor (403 below it) |
| `scope-claim` | `scope`, then `scp` | set `permissions` for Auth0 RBAC |
| `roles-claim` | `roles` + `realm_access.roles` | custom claim REPLACES defaults |
| `client-roles-for` | — | merge Keycloak `resource_access.<id>.roles` |
| `public-client-id`, `shim` | — / auto | shim on iff client id set; `shim: false` opts out |
| `registration-path` | `/oauth/register` | mounted under `mcp.path` |
| `redirect-uri-allowlist` | localhost/Claude/Inspector | custom list REPLACES defaults; `:*` = any port, trailing `*` = prefix |
| `extra-authorize-params` | — | map of query params merged into every authorization request (server wins over client duplicates); needs the shim — the mirror rewrites `authorization_endpoint` to `{mcp.path}/oauth/authorize`, which 302-redirects to the IdP |
| `filter-members` | `true` | hide unauthorized tools, resources and prompts from their list methods |
| `allow-insecure` | `false` | permit `http://` issuer/JWKS (dev only) |
| `mcp.allowed-origins` | — | transport-level Origin rejection list (DNS-rebinding guard) |
| `mcp.cors.allowed-origins` | Claude origins (+ localhost under `dev`) | origins granted browser CORS access |

### Provider matrix

| | issuer | DCR | audience | scopes / roles |
|---|---|---|---|---|
| **Keycloak** | `{base}/realms/{realm}` | anonymous DCR blocked → **shim** | add an **Audience mapper** (client scope) = the resource URI, else tokens carry `aud: ["account"]` → 401 | `scope`; roles from `realm_access` / `resource_access` (`client-roles-for`) |
| **Auth0** | `https://{tenant}.auth0.com/` (trailing slash!) | optional toggle | API identifier; inject `audience=` via `extra-authorize-params` (or `audience: any-of` + `extra-audiences` if the client sends it) | `scope`, or RBAC `permissions` → `scope-claim: permissions` |
| **Google** | `https://accounts.google.com` | none → **shim** | opaque access tokens → `token-validation: userinfo` (no `aud` binding — `audience: skip` is forced) | n/a |
| **Entra ID** | `…/{tenant}/v2.0` (path-insertion discovery handled) | none → **shim** | app-ID URI → `any-of` | `scp` claim (default ladder covers it); `roles-claim: roles` |
| **Okta** | `https://{org}.okta.com/oauth2/{as}` | gated → shim | `any-of` | `scp` (array form covered) |

### Keycloak walkthrough

1. Create a **public client** `mcp-public` in your realm: Standard flow on,
   PKCE `S256`, no client secret.
2. Redirect URIs on that client: `https://claude.ai/api/mcp/auth_callback`,
   `https://claude.com/api/mcp/auth_callback`, plus
   `http://localhost:*` for the Inspector.
3. Create a **client scope** `mcp` (default) with an **Audience mapper**
   whose included audience is your resource URI (e.g.
   `https://api.example.com/mcp`). Without it Keycloak issues
   `aud: ["account"]` and every token is rejected.
4. Optionally add scopes `mcp:read` / `mcp:write` (optional client scopes)
   and realm roles for `#[roles]`.
5. Configure R2E (the three keys at the top of this section).
6. `curl -i https://api.example.com/mcp` → `401` with
   `WWW-Authenticate: Bearer resource_metadata="…"` — the flow is live.
7. `npx @modelcontextprotocol/inspector`, or add the URL as a Claude custom
   connector: the client discovers the PRM, "registers" through the shim,
   runs authorization-code + PKCE against Keycloak, and calls tools.

### Testing authenticated servers (no Docker)

`r2e` feature `mcp-testing` (or `r2e-mcp` feature `testing`):

```rust
use r2e::r2e_mcp::testing::pin_mcp_validator;
use r2e_test::TestJwt;

const RESOURCE: &str = "http://localhost:3000/mcp";
let jwt = TestJwt::for_resource(RESOURCE);      // aud = resource (RFC 8707)
let app = pin_mcp_validator(AppBuilder::new(), &jwt, RESOURCE)  // BEFORE load_config
    .load_config::<()>()
    .plugin(McpServer::new())
    .build_state().await
    .register_mcp_service::<MathTools>();
let token = jwt.token_builder("alice").scopes(&["mcp:write"]).build();
```

The pinned validator (HS256, in-process) replaces the JWKS path via
`override_bean`, and the config overrides set `discovery: off` — **zero
network I/O at boot** while the real auth layer, well-known routes and
per-tool checks stay active. `TokenBuilder` mints every real-world token
shape: `.scopes()`, `.audiences()` (array `aud`), `.realm_roles()` /
`.client_roles()` (Keycloak), `.claim("scp", …)` (Entra/Okta), `.expired()`.
See the authenticated smoke in `examples/example-mcp/tests/mcp_e2e.rs` for
the complete wiring pattern; protocol and auth edge cases live in
`r2e-mcp/tests`.

For a real RS256/JWKS path without Docker, run the embedded `r2e-oidc` plugin
as the issuer: register the MCP client with
`ClientRegistry::add_public_client(...).with_scopes([...])`, configure the
audience to the MCP resource, and the local `/oauth/authorize` +
`/oauth/token` endpoints run Authorization Code + PKCE S256 end to end.
Clients are fail-closed — a client without `with_scopes` can only get an empty
scope, and anything outside its allowlist is rejected with `invalid_scope`.
The token endpoint honors the allowlisted `scope`
and accepts the matching RFC 8707
`resource` indicator (`resource=http://localhost:3000/mcp` → token `aud` =
the configured MCP resource). Any other resource is rejected with
`invalid_target`, so a shared issuer cannot mint tokens for arbitrary APIs. An
issued token then validates against `mcp.auth.issuer:
http://localhost:<port>` end to end. For a same-process issuer, pass
`oidc.claims_validator()` through `McpTokenValidator::jwt(...)` and set MCP
discovery to `off`; a validator override needs no dummy `mcp.auth.jwks-url`.
The explicitly enabled password grant remains available for non-browser
fixtures.

### Testing against a real Keycloak (Docker)

`r2e-devservices` feature `keycloak` ships `DevKeycloak`: a containerized
Keycloak importing a bundled MCP realm (`r2e-mcp`) — public client
`mcp-public` (PKCE S256, localhost + claude.ai redirect URIs) whose default
scope `mcp` stamps `http://localhost:3000/mcp` into the token `aud`, optional
scopes `mcp:read`/`mcp:write`, direct-grant client `test-cli`, confidential
`mcp-introspect` for RFC 7662, and users `alice` (admin) / `bob` (user).

```rust
let kc = DevKeycloak::shared().await;   // one container for the whole suite
let app = secured_plugin_app(McpServer::new().with_auth(McpAuthConfig {
    issuer: kc.issuer(),
    resource: Some("http://localhost:3000/mcp".into()), // = the realm's audience mapper
    allow_insecure: Some(true),
    ..Default::default()
})).await;
let token = kc.password_token("alice", "alice-password", "test-cli", "mcp:read").await;
```

This exercises the whole production path — discovery against the container,
RS256/JWKS, audience binding, scope checks, `tools/list` filtering. See
`r2e-mcp/tests/auth/keycloak.rs` (Docker-gated, `-- --ignored`).
