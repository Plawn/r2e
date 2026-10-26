# Migration: MCP mount stage and identity contract (r2e-mcp 0.9 → 0.10)

Released as the next minor of `r2e-mcp` and of the `r2e` facade (which
re-exports it); `r2e-macros` takes a codegen-contract bump cut with
`--no-propagate`. A `cargo update` after bumping `r2e` is the whole
dependency change.

Task #1175 makes the MCP endpoint an ordinary citizen of the HTTP app: it is
mounted like a controller route, so the app's middleware wraps it, and an
`#[inject(identity)]` parameter on an MCP member is extracted exactly like
one on an HTTP route. Two observable behaviours change.

## 1. `/mcp` is mounted in the Routes stage

**Before**: `McpServer` wrapped the finished router (Finalize stage). The
endpoint and the `mcp.auth` well-known routes sat outside every `add_layer`:
`HttpTrace`, metrics, the app's CORS and auth layers never saw them.
**After**: they are mounted in the Routes stage (`after_routes` +
`register_routes`). Every Graph layer wraps them like any other route.

What to check:

- **App-wide auth / rate-limit layers** installed with `add_layer` now gate
  `/mcp`, `/.well-known/oauth-protected-resource[{mcp.path}]`,
  `/.well-known/oauth-authorization-server` and the shim routes under
  `{mcp.path}/oauth/…`. If MCP clients must reach them with `mcp.auth`
  alone, exempt those paths in your layer. An MCP client cannot bootstrap
  OAuth if the protected-resource metadata is behind an app login.
- **An app CORS layer** now answers preflights (`OPTIONS /mcp`) before the
  MCP CORS layer sees them, so `mcp.cors.allowed-origins` no longer decides
  those. Configure the app layer for MCP browser clients:
  - allowed origins: the MCP client origins (`https://claude.ai`,
    `https://claude.com`, the Inspector's localhost origin…);
  - allowed request headers: `authorization`, `content-type`,
    `mcp-protocol-version`, `mcp-session-id`, `last-event-id`;
  - exposed response headers: `mcp-session-id`, `www-authenticate`.

  Missing any of these, a browser client cannot open or resume a session or
  read the auth challenge. Apps without their own CORS layer change nothing:
  the MCP CORS layer still decorates every MCP response, the auth 401
  included.
- **`HttpTrace` and metrics** now see MCP requests: one span and one request
  summary line per request (the long-lived SSE `GET` included), and
  `/mcp` in the HTTP metrics. Adjust dashboards or alerts that assumed
  otherwise.

**Delete your gateway plugin.** An app plugin that re-wrapped the MCP
endpoint so app layers would apply to it (a `McpGateway`-style
`wrap_router` re-mounting `/mcp` under the app's tracing/auth stack) is now
redundant: remove it and let the ordinary `add_layer` layers do the job.

## 2. `#[inject(identity)]` uses the HTTP extractor contract

**Before**: an `#[inject(identity)] user: T` parameter on an MCP member
compiled to `ToolCall::identity::<T>()`: a lookup of `Arc<T>`/`T` in the
request extensions. Under `mcp.auth`, `AuthenticatedUser` found the caller
there.
**After**: the parameter is extracted through `FromRequestPartsVia` against
the app state, exactly as on a `#[routes]` handler. `AuthenticatedUser` uses
its HTTP extractor: it validates the `Authorization` bearer itself with the
`Arc<JwtClaimsValidator>` bean and never reads the `mcp.auth` principal.

### Under `mcp.auth`: declare `McpPrincipal`

```rust,ignore
// Before
#[tool]
#[roles("admin")]
async fn whoami(&self, #[inject(identity)] user: AuthenticatedUser) -> String {
    user.sub
}

// After
#[tool]
#[roles("admin")]
async fn whoami(&self, #[inject(identity)] principal: McpPrincipal) -> String {
    principal.user.sub.clone()
}
```

`McpPrincipal` is in the prelude. `principal.user` is the caller (an
`Arc<AuthenticatedUser>`: reads go through `Deref`, an owned copy is
`(*principal.user).clone()`), `principal.scopes` are the granted scopes,
`principal.token_hash` a stable correlation key. It implements `Identity`
and `RoleBasedIdentity` by delegating to `principal.user`, so `#[roles]`,
`#[all_roles]` and custom guards behave as before. It also covers callers
that `AuthenticatedUser` from the JWT bean never could: opaque tokens
(`introspection`/`userinfo`) and HTTP Basic.

### Keep `AuthenticatedUser` only with the JWT bean

An `AuthenticatedUser` identity still compiles when the app provides the
`Arc<JwtClaimsValidator>` bean (the HTTP JWT setup). It then validates the
bearer on its own, with that validator's issuer/audience rules, not the
`mcp.auth` ones. Without the bean it is a compile error at the registration
call:

```text
error[E0277]: bean `Arc<JwtClaimsValidator>` is not present in the application state
  --> src/main.rs:27:37
   |
27 |             .register_mcp_service::<WhoAmITools>()
   |              --------------------   ^^^^^^^^^^^ `Arc<JwtClaimsValidator>` was never provided or registered on the AppBuilder
   ...
   = note: required for `AuthenticatedUser` to implement `FromRequestPartsVia<BeanState<…>, ViaBean<There<_>>>`
note: required for `WhoAmITools` to implement `McpService<BeanState<…>, (ViaBean<There<_>>,)>`
```

Read that error as "switch this parameter to `McpPrincipal`" when the app
authenticates MCP through `mcp.auth`.

### App identity types work as is

Any type implementing `FromRequestParts` (or a bean-backed
`FromRequestPartsVia`) is accepted, exactly as on an HTTP route. Since app
layers now wrap `/mcp` (section 1), an identity that reads an extension an
app layer inserted works on MCP members too.

### `Option<_>`

`Option<T>` runs the type's optional extractor: `None` when the caller is
absent. For `Option<McpPrincipal>` that means no principal on the request;
for `Option<AuthenticatedUser>`, no `Authorization` header. A present but
invalid credential is still the extractor's rejection. A rejection becomes an
`McpError` by kind: 401 → JSON-RPC `unauthorized`, 403 → `forbidden`. Order
per call: scope check → identity → guards → params → method.

### Hand-written `McpService` impls

`McpService` is state-generic, the MCP peer of `Controller<T, W>`, and
`routes` receives the app state:

```rust,ignore
// Before
impl McpService for MyTools {
    fn service_name() -> &'static str { "MyTools" }
    fn routes(ctx: &Arc<BeanContext>) -> McpRoutes { /* … */ }
}

// After: no identity extraction, so the witness stays `()`
impl<T: Clone + Send + Sync + 'static> McpService<T> for MyTools {
    fn service_name() -> &'static str { "MyTools" }
    fn routes(_state: &T, ctx: &Arc<BeanContext>) -> McpRoutes { /* … */ }
}
```

`AppBuilderMcpExt` gained a witness too (`AppBuilderMcpExt<T, W, DepIdx>`);
call sites stay `.register_mcp_service::<MyTools>()` and never name it.
`#[mcp_routes]` services need no change beyond the identity types above.

### Unchanged

- `ToolCall::identity::<T>()` / `extension::<T>()` (and the
  `ResourceCall`/`PromptCall`/`Completion` peers), and
  `SessionInit::identity::<T>()` still read the request extensions, for
  members that take the raw call.
- Session subject binding, `tools/list` filtering and the `#[tool(scopes)]`
  check still read the `mcp.auth` principal.

Full guide: `docs/features/25-mcp.md` § "Identity" and § "Configuration"
("Mounting and app middleware", "Double CORS").
