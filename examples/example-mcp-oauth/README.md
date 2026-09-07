# example-mcp-oauth

The smallest useful authenticated MCP server: **one tool** (`whoami`) behind a
real OAuth 2.1 resource server, published through a public tunnel so Claude
Code, claude.ai and ChatGPT can connect to it as a custom connector.

There is no auth code in `src/app.rs` — the whole resource server comes from
two keys in `application.yaml` (`mcp.auth.issuer` + `server.public-url`).

| | |
|---|---|
| Tool | `whoami` — echoes the sub / username / email / roles / scopes of the access token the client presented |
| IdP | any OIDC issuer (the walkthrough below uses Keycloak) |
| Local port | `3333` (override with `PORT`) |

Everything deployment-specific is a `${VAR:default}` placeholder:

```bash
cd examples/example-mcp-oauth
cp .env.example .env      # then fill in issuer, client id and tunnel URL
```

## Run

The config file is resolved from the **current directory**, so start from this
crate's directory — running from the workspace root would pick up the repo's
own `application.yaml` and silently boot an unauthenticated server on `:3000`:

```bash
cd examples/example-mcp-oauth
cargo run -p example-mcp-oauth
```

In another terminal:

```bash
ngrok http 3333 --url your-tunnel.ngrok-free.dev
```

Boot should log:

```
MCP OAuth resource server enabled resource=https://…/mcp shim=true discovery=Eager
Mounting MCP endpoint path=/mcp tools=["whoami"]
```

`discovery=Eager` succeeding means R2E actually reached the issuer's
`.well-known/openid-configuration` at boot — a typo in the issuer is a boot
failure, not a runtime 401.

## IdP checklist (Keycloak)

The DCR shim **registers nothing**: it hands every MCP client the id from
`mcp.auth.public-client-id`, so that client must already allow the callbacks
below.

1. **Public client**, Standard flow on, no client secret, PKCE `S256`.
2. **Valid redirect URIs** — add whichever clients you want to test:
   - `https://claude.ai/api/mcp/auth_callback` (claude.ai web)
   - `https://claude.com/api/mcp/auth_callback`
   - `http://localhost:*` (Claude Code CLI, MCP Inspector)
   - `https://chatgpt.com/connector_platform_oauth_redirect` (ChatGPT connectors)
3. **Web origins**: `https://claude.ai`, `https://chatgpt.com` (or `+`).
4. *(recommended, see below)* a dedicated **Audience mapper**.

### The audience caveat

`.env.example` ships with `MCP_AUDIENCE_MODE=skip`, which is what makes this
work with **zero IdP changes** — and it is the one thing here that is not
production-grade: any token that issuer minted for any service authenticates.

R2E's default (`audience: resource`) requires the token's `aud` to contain the
canonical resource URI `{server.public-url}/mcp` (RFC 8707). Keycloak only puts
it there if you add, on the public client, a **dedicated mapper of type
"Audience"** whose *Included Custom Audience* is exactly that URI. Without the
mapper Keycloak issues `aud: ["account"]` and every request is a 401.

Once the mapper exists, set `MCP_AUDIENCE_MODE=resource`. Note that the
resource URI contains the tunnel hostname, so a new tunnel URL means updating
the mapper too.

## Connect a client

### Claude Code

```bash
claude mcp add --transport http my-mcp https://your-tunnel.ngrok-free.dev/mcp
```

Then `/mcp` in the session → *Authenticate*. The browser opens the IdP, the
callback lands on `http://localhost:<port>`, and `whoami` shows up in the tool
list.

### claude.ai / Claude Desktop

Settings → Connectors → *Add custom connector* → the same `/mcp` URL. Claude
discovers the protected-resource metadata from the 401 challenge, "registers"
through the shim, and runs authorization-code + PKCE against the IdP.

### ChatGPT

Settings → Connectors → *Create* (developer mode must be enabled) → MCP server
URL = the same `/mcp` URL → authentication *OAuth*. ChatGPT reads the same PRM
and shim.

### MCP Inspector

```bash
npx @modelcontextprotocol/inspector
```

Transport *Streamable HTTP*, the same URL, OAuth flow.

## Verifying by hand

```bash
BASE=https://your-tunnel.ngrok-free.dev

# 401 + the RFC 9728 challenge that bootstraps every client
curl -i -X POST $BASE/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'

# the metadata that challenge points at
curl -s $BASE/.well-known/oauth-protected-resource/mcp

# the mirrored AS metadata (registration_endpoint rewritten to the shim)
curl -s $BASE/.well-known/oauth-authorization-server
```

## Troubleshooting

| Symptom | Cause |
|---|---|
| OAuth completes, then the client reports a connection failure; the log shows `rejected request with disallowed Host header` | `mcp.allowed-hosts` doesn't list the tunnel hostname. The Host check sits *inside* rmcp, **after** the auth layer — so this failure mode always looks like a post-login error, never like an auth error |
| 401 `invalid audience` | `audience: resource` without the IdP audience mapper — see above |
| Client can't register | The redirect URI it sent isn't in `mcp.auth.redirect-uri-allowlist` **and/or** isn't configured on the public client at the IdP |
| Server boots unauthenticated on `:3000` | Launched from the workspace root: the repo's own `application.yaml` won |
| `whoami` returns `unauthorized` | The identity parameter is required by design — the token didn't validate |
