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
