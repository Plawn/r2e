# Migration: MCP global registry (r2e-mcp 0.6 → 0.7)

Released as `r2e-mcp` 0.7.0 and `r2e` 0.7.0 (the facade re-exports the crate);
`r2e-core` and the other crates stay on 0.6.x — a plain `cargo update` after
bumping `r2e` to `0.7` is the whole dependency change.

Task #1073 makes every MCP list dynamic: a plugin-provided `McpRegistry` bean
can add, replace or remove tools, resources and prompts after boot, for every
session and every sessionless caller. Four observable behaviours change with
it; none needs more than a few lines.

## 1. The endpoint is always mounted

**Before**: with no `.register_mcp_service::<T>()` the plugin logged a warning
and mounted nothing (`POST /mcp` → 404).
**After**: the endpoint is mounted and serves empty lists until the registry
adds a member.

If an app relied on the endpoint being absent, say so explicitly:

```yaml
mcp:
  enabled: false
```

## 2. Capabilities are always advertised

**Before**: `resources` / `prompts` (and `completions`) were advertised only
when the catalog held such a member, a session-init hook was installed, or
sessions were enabled.
**After**: `tools`, `resources` (`subscribe`) and `prompts` are always
advertised with `listChanged: true`, plus `completions`.

Nothing to change on the server. A test asserting a capability is *absent*
asserts the new shape instead:

```rust,ignore
for family in ["tools", "resources", "prompts"] {
    assert_eq!(capabilities[family]["listChanged"], true);
}
```

## 3. `McpServer::Provided` is a 4-tuple

**Before**: `(McpTokenValidator, McpResourceUpdates, McpSessions)`.
**After**: `(McpTokenValidator, McpResourceUpdates, McpSessions, McpRegistry)`.

Only code that destructures the plugin's provisions (a custom plugin wrapping
`McpServer`, a test pinning the tuple) is affected — add the fourth element.
`#[inject] registry: McpRegistry` now resolves anywhere the plugin is
installed.

## 4. `McpSessionError` → `McpMemberError`

The error type is shared by `McpSession::add_*` and `McpRegistry::add_*`, so
it is named after the member, not the session. `McpSessionError` remains as a
type alias; `use` the new name when touching the code:

```rust,ignore
use r2e::prelude::McpMemberError;   // was McpSessionError
```

## Also in this release (additive)

- `ToolRoute` / `ResourceRoute` / `PromptRoute` have a public `meta` field.
  Code building a route literal by hand adds `meta: None`; `#[mcp_routes]`
  and the `Dynamic*` builders fill it for you.
- The 401 body for a request with no `Authorization` header says
  `missing credentials` (was `missing bearer token`), since HTTP Basic
  (`mcp.auth.basic`) is now a second scheme.

## What the registry gives you

```rust,ignore
#[derive(Clone)]
pub struct PackLoader { mcp: McpRegistry }

#[bean]
impl PackLoader {
    pub fn new(mcp: McpRegistry) -> Self { Self { mcp } }

    pub fn load(&self, pack: &Pack) -> Result<(), McpMemberError> {
        self.mcp.edit(|edit| {               // atomic; ONE list_changed per touched family
            for tool in pack.tools() { edit.upsert_tool(tool)?; }
            Ok(())
        })
    }
}
```

Precedence is **catalog > session-private > registry**: a `#[mcp_routes]` key
can never be taken by a dynamic member (`Err`, or a boot panic for a member
added before the mount), and a session-private member shadows a registry
namesake for that session only. Full guide: `docs/features/25-mcp.md`
§ "For everyone — the `McpRegistry` bean".
