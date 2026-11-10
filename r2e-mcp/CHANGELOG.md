# Changelog — r2e-mcp

All notable changes to `r2e-mcp` are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the version is this
crate's own (pre-1.0: a breaking change bumps the minor, anything else the
patch).

Write entries under `[Unreleased]` in the PR that makes the change. A release
PR (`scripts/bump-version.sh`) turns that section into `[X.Y.Z] - date`, and
`.github/workflows/release.yml` publishes the crate, tags `r2e-mcp-vX.Y.Z` and
uses the section as the GitHub release notes.

Up to 0.6.0 the whole workspace was released in lockstep; that history lives
in the root [`CHANGELOG.md`](../CHANGELOG.md).

## [Unreleased]

## [0.12.0] - 2026-11-10

### Added

- `McpServer::preset(McpConfig)` (#1291): a library's defaults for the `mcp.*`
  section, below the application file, itself below the builder setters. The
  plugin resolves the three tiers through `r2e_core::config::Layered`
  (`mcp.auth`, a section with required keys, is taken whole from the highest
  tier that sets it — as before); observable precedence of the existing
  setters is unchanged.

### Changed

- `mcp.allowed-origins` and `mcp.cors.allowed-origins` use the origin grammar
  and matcher shared with the `Cors` plugin
  (`r2e_http::layers::{validate_origin, origin_allowed, allow_origins}`,
  #1288). Every entry is validated at boot: a malformed origin (path, query,
  missing scheme, empty host, non-numeric port) is a `PluginBuildError`
  naming the key instead of an entry that silently never matches. Valid
  configurations behave as before; `scheme://host:` (empty port) no longer
  matches `scheme://host:*`.

### Fixed

- A struct-level identity rejected with a 401 that already carries a `Bearer`
  challenge (`AuthenticatedUser` since #1173) gets `resource_metadata="…"`
  merged into that challenge (RFC 9728 §5.1) instead of being left without
  the PRM URL; the `mcp.auth` Bearer / Basic challenges are only appended
  when the rejection carries none of that scheme. The values are parsed as
  RFC 9110 challenge lists first (several challenges in one value, quoted
  commas, case-insensitive scheme and parameter names, whitespace around
  `=`), so a `Bearer …, Basic …` value is completed on the Bearer and the
  Basic is not duplicated.

### Added

- Feature `loopback` (#1076): an in-process transport on the MCP endpoint.
  The plugin provides the `McpLoopback` bean; `connect(session_id,
  LoopbackIdentity)` opens a real session (same handler, catalog,
  `McpSessions`, guards, roles, scopes, `McpSessionInit`) and returns a
  `LoopbackSession` (derefs to rmcp's `RunningService<RoleClient, ()>`).
  Identities: `Anonymous`, `Bearer` (validated by the plugin's
  `McpTokenValidator`, `required-scopes` applied) and a trusted `Principal`;
  struct-level request scopes run on the synthesized request. Errors are
  `LoopbackError`. Compiles rmcp's `client` role — no new crate — and is off
  by default. `McpServer::Provided` gains a 5th element (`McpLoopback`) under
  the feature only.
- `McpPrincipal::trusted(user, scopes)`: a principal the process vouches for
  (no token; `token_hash` is `0`).
- `r2e_mcp::CallToolRequestParams` re-export (feature `loopback`) for
  loopback callers.
- `mcp.session.idle-timeout-secs` / `McpServer::with_session_idle_timeout`
  (#1244): the Streamable HTTP session idle timeout (rmcp's
  `SessionConfig::keep_alive`, previously hard-wired to 300 s; `0` = never),
  so a tool call that stays silent longer than 5 minutes is no longer cut
  mid-flight. New `McpSessionConfig` section type. Loopback sessions are
  unaffected. Docs: `docs/features/25-mcp.md` § Configuration.
- `McpSessionStore` (#1244): pluggable persistence of Streamable HTTP
  sessions, so an `Mcp-Session-Id` survives a restart, an idle eviction or a
  hop to another instance behind a load balancer. `load`/`store`/`delete`
  over opaque bytes with a TTL (`McpSessionStoreError`,
  `McpSessionStoreFuture`); set with `McpServer::with_session_store` or an
  `Arc<dyn McpSessionStore>` bean; `InMemoryMcpSessionStore` for dev/tests.
  The handshake and the owning subject are written at `initialize` and
  re-armed at half-life of `mcp.session.store-ttl-secs`
  (`McpServer::with_session_store_ttl`, default 1800; `0` and `stateless` +
  store are boot errors). A restored session runs `McpSessionInit` again
  with `SessionInit::is_restored()`; only the subject that opened it can
  restore or delete it (rmcp's `404` otherwise, no oracle). Loopback
  sessions are never written. Docs: `docs/features/25-mcp.md` § "Session
  persistence".

### Changed

- Built against `r2e-core` 0.10.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.10.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-security` 0.11.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.11.0/r2e-security/CHANGELOG.md)).
- Built against `r2e-test` 0.11.0 — its breaking changes surface through this crate's re-exports (see [`r2e-test/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-test-v0.11.0/r2e-test/CHANGELOG.md)).

## [0.11.0] - 2026-11-04

### Breaking

- MCP services are declared with `#[routes]` (#1215, via `r2e-macros`):
  an impl whose members are `#[tool]`/`#[resource]`/`#[prompt]`/
  `#[completion]` is an `McpService`. One type exposes one port: MCP members
  next to HTTP routes are a compile error — serve HTTP and MCP from two types
  over the same beans. **`#[mcp_routes]` is removed** (no alias).
  Migration: rename `#[mcp_routes(...)]` to `#[routes(...)]` — same
  arguments, same members, same generated code; an MCP service with no
  member is no longer expressible (an impl with no member reads as HTTP).
  Docs: `docs/features/25-mcp.md` § One port per type, `llm/mcp-server.md`.
  - `McpService` trait: new required `fn construct(state: &T, ctx:
    &BeanContext) -> Self`, and `routes` takes the core —
    `fn routes(state: &T, core: Arc<Self>, ctx: &Arc<BeanContext>) ->
    McpRoutes`; the builder takes the core from its per-type core store
    (constructed through `construct` on first use). `#[routes]` generates
    both; a hand-written impl moves its `from_context` call into `construct`
    and uses the `core` argument.
  - `AppBuilderMcpExt::register_mcp_service` / `try_register_mcp_service`
    now also require `S: Send + Sync` (the stored core is shared).

- MCP members now run on the request façade, exactly like HTTP handlers
  (#1210). In a member body `Self` names the façade:
  name an associated item of the service through the service type
  (`MyTools::HELPER()`, not `Self::HELPER()`), and a member whose signature
  exposes `Self` (`-> Self`, `Option<Self>`, …) is a compile error naming the
  rule. Members that only use `self.<field>` and `self.<method>()` are
  unaffected.
- The `McpService<S, W>` witness `W` starts with the request-data marker: a
  service without identity is `McpService<S, ((),)>`, no longer
  `McpService<S, ()>`. `.register_mcp_service::<Svc>()` infers it; only
  hand-written bounds (`S: McpService<State, ()>`) need to become generic
  over `W` (`S: McpService<State, W>, W`).

### Added

- **Struct-level identity: one caller for the whole endpoint** (#1210).
  `#[inject(identity)]` and `#[inject(request)]` *fields* on an MCP
  service follow the `#[controller]` contract (same
  attributes, same extractors, `#[request_helper]`, `self.<field>` in every
  member) and are extracted **once per HTTP request** in a layer on `/mcp`,
  before rmcp parses the body: the endpoint fails closed as a whole —
  `initialize` included, no session opened — with the extractor's own HTTP
  rejection; `Option<T>` admits anonymous requests with `None`. Every
  registered service's scope runs on every request; `OPTIONS` is skipped.
  - `#[roles]`/`#[all_roles]` on a member check the struct identity
    (`RoleBasedIdentity`); the member lists are filtered for it, as under
    `mcp.auth` (`mcp.auth.filter-members` keeps its meaning when `mcp.auth`
    is on).
  - A session is bound to the struct identity's subject (`Identity`): another
    subject presenting it gets `404 Not Found`, as with the `mcp.auth`
    principal — never both bindings on one request.
  - Under `mcp.auth` the field may be `McpPrincipal` (the deposited
    principal, validated once). An app identity's bare `401` gets the
    `mcp.auth` `WWW-Authenticate` challenge appended.
  - Request-scoped field types must be `Clone` (each member binds its façade
    from the deposited data); the check lands at
    `.register_mcp_service::<Svc>()`.
  - Internals, `#[doc(hidden)]` under `__macro_support`: `RequestScope`,
    `deposit_request_data`/`request_data`, `CallerProbe` and `McpCaller`.
  - Docs: `docs/features/25-mcp.md` § Identity → *Struct-level identity*,
    `llm/mcp-server.md`.

### Fixed

- More `mcp.*` keys fail fast at boot instead of misbehaving at request time
  (#1204). The error names the key and the offending value; the config types
  are unchanged.
  - `mcp.auth.scopes-supported` / `mcp.auth.required-scopes`: an empty entry,
    or a character outside the RFC 6749 §3.3 scope-token grammar (space, `"`,
    `\`, control or non-ASCII), is rejected. An entry like `"openid profile"`
    used to reach clients as-is and make IdPs answer `invalid_scope`.
  - `mcp.auth.jwks-url`, `authorization-endpoint`, `token-endpoint`,
    `userinfo-endpoint`, `introspection-endpoint`: must be an absolute URL
    with a host, and `https` unless `mcp.auth.allow-insecure: true`. An
    empty `jwks-url` (what `${MCP_JWKS_URL:}` resolves to) still means
    "resolve from discovery"; an empty explicit endpoint is a boot error.
  - `mcp.allowed-hosts` (and `McpServer::with_allowed_hosts`): an empty list,
    an empty entry, or an entry with leading/trailing whitespace is rejected.
    rmcp reads an empty list as "Host check off", so `[]` (or a `${VAR}` that
    resolved to nothing) used to turn off the DNS-rebinding protection
    without saying so; a padded entry never matches a `Host` header and
    would 403 every request.

### Added

- Auth observability (#1206): an OAuth flow that fails can be diagnosed from
  the server logs. No line carries a token.
  - Every refused credential logs one WARN `MCP request rejected` with its
    `cause` (`malformed_header`, `malformed_token`, `missing_kid`,
    `unknown_kid`, `algorithm_refused`, `invalid_signature`,
    `token_expired`, `claims`, `insufficient_scope`, `jwks_unavailable`, …)
    and the validator's `detail`. A request without credentials, the normal
    first leg of every OAuth flow, is DEBUG. The wire `error_description`
    stays coarse.
  - One DEBUG `MCP request` line per request: JSON-RPC method, HTTP status,
    auth outcome. The method is taken from the bytes rmcp itself reads (the
    body is tapped, never buffered ahead of it; bounded to the known MCP
    methods), so a request refused on its headers or credentials costs no
    body read. It complements `HttpTrace`'s summary and runs inside its span.
  - One line per DCR shim call: INFO on success, WARN on a refusal or dropped
    redirect URIs. The line is bounded whatever the unauthenticated caller
    sends: `client_name` is clipped, dropped URIs appear as a count plus a
    short clipped sample.
  - A validator that never got initialised (`after_build` did not run) is one
    WARN `MCP request rejected` with cause `validator_not_initialised`, not a
    bare ERROR beside it.
  - The DCR shim's registration response echoes the advertised scopes as
    `scope`.
  - Boot WARNs when the DCR shim is on and the protected-resource metadata
    advertises no scope, naming `mcp.auth.scopes-supported`: clients then
    pick a scope of their own and the IdP answers `invalid_scope`.

### Changed

- The `register_mcp_service` configuration-error panic renders through
  `r2e_core::config::ConfigErrorReport` (#1214); the message is
  byte-identical.

- Built against `r2e-security` 0.10.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.10.0/r2e-security/CHANGELOG.md)).

## [0.10.0] - 2026-10-31

### Breaking

- `McpServer` mounts `/mcp` and the `mcp.auth` well-known routes in the
  **Routes** stage (`after_routes` + `register_routes`) instead of wrapping
  the finished router (Finalize). Every app Graph layer installed with
  `add_layer` now wraps the endpoint: `HttpTrace` and metrics see MCP
  requests, and app-wide auth / rate-limit layers now gate `/mcp` and the
  well-known routes (exempt them there if they must stay reachable with
  `mcp.auth` alone). An app CORS layer now answers MCP preflights before the
  MCP one: it must allow the request headers `authorization, content-type,
  mcp-protocol-version, mcp-session-id, last-event-id` and expose
  `mcp-session-id, www-authenticate` (#1175).
- `McpService` is state-generic: `McpService<T: Clone + Send + Sync +
  'static, W = ()>` with `fn routes(state: &T, ctx: &Arc<BeanContext>) ->
  McpRoutes` (was `routes(ctx)`). Hand-written impls take the state
  argument; `#[mcp_routes]` impls are regenerated (#1175).
- `AppBuilderMcpExt<T, DepIdx>` → `AppBuilderMcpExt<T, W, DepIdx>` (bound
  `S: McpService<T, W>`). Call sites are unchanged
  (`.register_mcp_service::<Svc>()`); only code naming the trait's generics
  is affected (#1175).
- `#[inject(identity)]` on `#[tool]`/`#[resource]`/`#[prompt]`/`#[completion]`
  members is extracted through `FromRequestPartsVia` against the app state —
  the HTTP route contract — instead of being read from the request
  extensions. Under `mcp.auth` an `AuthenticatedUser` identity no longer
  reads the validated principal (it uses its HTTP extractor and needs the
  `Arc<JwtClaimsValidator>` bean, a compile error at `register_mcp_service`
  without it): declare `McpPrincipal` and read `principal.user`. Migration:
  [`docs/migration/mcp-identity.md`](../docs/migration/mcp-identity.md)
  (#1175).

### Added

- `mcp.max-protocol-version` / `McpServer::max_protocol_version(ProtocolVersion)`:
  cap the MCP revisions the server speaks. `server/discover` lists only the
  revisions up to the cap, `initialize` negotiates within it and a sessionless
  request whose `_meta` names a newer revision fails with `-32022` and the
  supported list. Unknown values fail at boot naming the known revisions.
  `ProtocolVersion` is re-exported at the crate root (#1183).
- `McpPrincipal` and `Option<McpPrincipal>` extract as `#[inject(identity)]`
  under the new `ViaMcpAuth` marker (re-exported at the crate root); a
  required principal missing from the request is a JSON-RPC `unauthorized`
  error naming `mcp.auth` (#1175).
- `McpPrincipal` implements `Identity` and `RoleBasedIdentity` (delegating to
  `principal.user`), so `#[roles]` and custom guards work on it (#1175).
- `McpPrincipal` is in `r2e_mcp::prelude` (and the facade prelude) (#1175).
- MCP members accept any identity an HTTP route accepts: app
  `FromRequestParts` types (including ones reading extensions an app layer
  inserted) and bean-backed extractors, checked at
  `register_mcp_service` (#1175).

### Changed

- Minimum `rmcp` is 3.8 (was 3.1): `initialize` negotiation is delegated to
  rmcp's `negotiate_initialize`, and `2026-07-28` (sessionless,
  `server/discover`) is served by default (#1183).
- The rmcp noise rules are contributed as `Suppress` tracing layers through
  `PluginSetupContext::tracing_layer` instead of the removed process-global
  `r2e_core::suppress_error` (see `docs/migration/tracing-single-path.md`).

### Fixed

- A stateless client disconnecting first (a `subscriptions/listen` SSE stream
  closed after the client's idle period) no longer logs
  `failed to send pending response during drain` at ERROR under R2E's tracing
  subscriber: rmcp ≤ 3.8 reports the closed oneshot channel of the per-request
  service at that level. `McpServer::setup` contributes a
  `Suppress::error` layer per `RMCP_NOISE` entry
  (`PluginSetupContext::tracing_layer`), installed with the subscriber by
  `build_state()`.
  Nothing leaks. A canary test pins the upstream behaviour so the rule is
  removed with the rmcp fix (#1185).

### Changed

- Built against `r2e-core` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.9.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-security` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.9.0/r2e-security/CHANGELOG.md)).
- Built against `r2e-test` 0.9.0 — its breaking changes surface through this crate's re-exports (see [`r2e-test/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-test-v0.9.0/r2e-test/CHANGELOG.md)).

## [0.9.0] - 2026-10-25

### Changed

- Built against the `Routes` newtype of `r2e-core` / `r2e-http` (breaking
  there: `docs/migration/routes-newtype.md`); this crate's own API is
  unchanged.

- The MCP endpoint's CORS layer is built from `r2e_core::http::layers::cors`;
  this crate no longer depends on `tower-http` directly. No behaviour change.

- Built against `r2e-core` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.8.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-test` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-test/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-test-v0.8.0/r2e-test/CHANGELOG.md)).
- Built against `r2e-security` 0.8.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.8.0/r2e-security/CHANGELOG.md)).

## [0.8.0] - 2026-10-12

### Changed

- Built against `r2e-core` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-core/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-core-v0.7.0/r2e-core/CHANGELOG.md)).
- Built against `r2e-security` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-security/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-security-v0.7.0/r2e-security/CHANGELOG.md)).
- Built against `r2e-test` 0.7.0 — its breaking changes surface through this crate's re-exports (see [`r2e-test/CHANGELOG.md`](https://github.com/Plawn/r2e/blob/r2e-test-v0.7.0/r2e-test/CHANGELOG.md)).

## [0.7.0] - 2026-10-11

### Breaking

The endpoint gains a dynamic member registry (task #1073); the developer
migration is [`docs/migration/mcp-registry.md`](../docs/migration/mcp-registry.md).

- **Breaking: the MCP endpoint is mounted even with no `#[mcp_routes]`
  service** (`McpRegistry` can add every member after boot). An app that
  relied on the endpoint being absent must set `mcp.enabled: false`.
- **Breaking: `tools`, `resources` and `prompts` capabilities are always
  advertised** with `listChanged: true` (plus `completions`), stateless or
  not, whether or not the catalog holds a member of that family.
- **Breaking: `McpServer::Provided` grows to
  `(McpTokenValidator, McpResourceUpdates, McpSessions, McpRegistry)`.**
  Only code destructuring the plugin's provisions is affected.
- **Breaking: `McpSessionError` is renamed `McpMemberError`** (the error is
  shared by sessions and the registry); the old name stays as a type alias.
- **Breaking: `ToolRoute` / `ResourceRoute` / `PromptRoute` gain a public
  `meta` field** — struct literals building a route by hand add `meta: None`.
- The 401 reason for a request with no `Authorization` header reads
  `missing credentials` (was `missing bearer token`), now that HTTP Basic is a
  second scheme.

### Added

- **`McpRegistry`**: a plugin-provided bean to add, upsert or remove tools,
  resources and prompts every session — and every sessionless caller — serves
  after boot; `edit(|e| …)` applies a batch atomically and emits one
  `list_changed` per touched family to every live session and
  `subscriptions/listen` stream, with no per-session rebuild (the handler
  merges the session view and the registry view at request time). Precedence
  catalog > session-private > registry: a catalog key is never overridable
  (add fails; a pre-mount clash panics at boot); a session-private member
  shadows a registry namesake for that session only. Works before the mount,
  under `mcp.stateless` and with `mcp.enabled: false`.
- **`_meta` on MCP members**: `#[tool(meta = meta!({ … }))]` (also
  `#[resource]`, `#[prompt]`) and `DynamicTool/Resource/Prompt::meta(..)` pass
  a free-form `_meta` object through verbatim in the list entries (MCP Apps
  bind a tool to its `ui://` widget with `_meta.ui.resourceUri`); the `meta!`
  macro and `MetaObject` are re-exported from the prelude.
- **HTTP Basic auth** (`mcp.auth.basic.{username,password,realm,roles,scopes}`):
  stands alone (no issuer ⇒ no PRM/discovery/shim, `WWW-Authenticate: Basic`
  challenge) or composes with OAuth 2.1 (the `Authorization` scheme selects
  the path). The principal is built once at boot; compares are constant-time.
- **Transport session id on calls**: `ToolCall::session_id()` (also
  `ResourceCall`, `PromptCall`, `McpSession::id()`, `SessionInit::session_id()`)
  exposes the `Mcp-Session-Id` as a `SessionId`; `McpSessions::get(&id)` finds
  the live session back.
