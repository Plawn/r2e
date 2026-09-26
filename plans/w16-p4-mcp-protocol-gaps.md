# W16 P4 — MCP protocol gaps (progress, elicitation, completion, pagination)

Status: **§1 Progress DONE** (f065a3e4), **§2a Elicitation DONE** (2026-09-26, branch `feat/mcp-dynamic-session-members`); §3 next. Origin:
W16 "what is left" review — `r2e-mcp` answers the member surface (tools /
resources / templates / prompts / subscriptions / list_changed) but five
protocol areas are either rmcp's default stub or absent.

**Scope decision (user, 2026-09-26): nothing deprecated by SEP-2577 is
implemented** — no sampling, no roots, no logging (`logging/setLevel` /
`notifications/message`). Only progress, elicitation, completion and
pagination are in scope.

Wire layer: rmcp 3.1.4 (`server`, `transport-streamable-http-server`). All
rmcp facts below were checked against that source.

## 0. Cross-cutting constraints

| # | Constraint | Consequence |
|---|---|---|
| C1 | **SEP-2577** deprecates server-initiated `sampling/createMessage`, `roots/list` and `notifications/message` (logging). rmcp marks `create_message`, `list_roots`, `notify_logging_message` `#[deprecated]` and says they "will be removed in a future release". | **Not implemented** (scope decision above): no API, no feature flag, no capability advertised; `logging/setLevel` keeps rmcp's `method_not_found`. |
| C2 | **SEP-2260** (2026-07-28): a server→client request must be issued *while handling* a client request. rmcp tracks the association in a task-local (`OriginatingRequestId`) — it does **not** cross `rt::spawn` — and routes the request onto the originating POST's SSE stream. | Server→client handles are only valid inside the member future. Never `Clone + 'static` handles that outlive the call. |
| C3 | **MRTR** (SEP-2322, 2026-07-28 sessionless): instead of a live server→client request, a member returns `InputRequiredResult { input_requests, request_state }`; the client retries with `input_responses` + echoed `request_state`. Only peers that negotiated ≥ 2026-07-28 accept it. `request_state` is client-controlled. | Two execution models for the same member. `request_state` must be sealed (rmcp `request-state` feature / `RequestStateCodec`) or be a server-side handle. |
| C4 | `mcp.json-response: true` turns POST replies into plain JSON — no SSE stream, so in-request notifications (progress, log) and in-request server→client requests have nowhere to go. | Degrade to no-op (notifications) / typed error (requests), documented. Detect at boot where possible. |
| C5 | `mcp.stateless: true` has no session. | Per-session state (log level) falls back to a default; live peer requests need the POST stream only (fine), MRTR works. Same boot-check pattern as `McpSession` (`catalog.rs:484`). |
| C6 | Visibility is the security boundary: a member hidden from the caller (requirements, disabled group, other session's private member) must answer exactly like an unknown one. | Every new entry point (`completion/complete`, paginated lists) goes through `SessionView` + `visible_list`, never `Catalog` directly. |
| C7 | Member params are a closed `McpToolArg` set parsed in `r2e-macros/src/parsing/mcp_routes_parsing.rs` (`Identity`, `Params`, `Call`, `Cancel`, `Session`). | New capabilities = new `McpToolArg` variants resolved from `ToolCall`/`ResourceCall`/`PromptCall`, never reflection. Plus compile-tests for misuse. |

Every phase: `llm/mcp-server.md` + `docs/features/25-mcp.md` updated,
`scripts/check-llm-docs.sh --update`, `cargo test -p r2e-mcp --features testing`,
`cargo test -p llm-doctests`, new trybuild cases in `r2e-compile-tests`,
tests in `r2e-mcp/tests/server/` (new `mod`s, no new top-level targets).

---

## 1. Progress — `notifications/progress`

**Why first:** smallest, not deprecated, useful to every long tool, and it
establishes the "peer handle inside the call" plumbing reused by §2.

### API

```rust,ignore
#[tool(description = "Reindex the catalog")]
async fn reindex(&self, progress: Progress) -> Result<String, McpError> {
    let items = self.repo.all().await?;
    for (i, item) in items.iter().enumerate() {
        self.index(item).await?;
        progress.report(i as f64 + 1.0, Some(items.len() as f64), None).await;
    }
    Ok("done".into())
}
```

- `pub struct Progress` (in `route.rs` or new `progress.rs`):
  - `report(progress: f64, total: Option<f64>, message: Option<&str>)` — async, infallible (errors logged at `debug`, never surfaced: progress is advisory).
  - `is_requested() -> bool` — client sent a `progressToken`.
  - No token ⇒ every call is a no-op (spec: server MUST NOT send progress without a token).
  - Monotonicity: spec requires strictly increasing `progress`. Keep last value (`AtomicU64` bits); drop non-increasing reports with a `debug!` instead of sending invalid wire.
  - Rate cap: optional `mcp.progress.min-interval` (default 0 = off) — later, only if needed.
- `Progress` is `!Send`-agnostic but deliberately **not `'static`-escapable** in docs: it holds a `Peer<RoleServer>` clone, so technically it could be moved into a spawned task. Notifications are not bound by SEP-2260 (only requests are), so a spawned reporter still works on legacy SSE; document that the POST stream closes when the member returns, after which reports are dropped.

### Plumbing

- `ToolCall`/`ResourceCall`/`PromptCall` gain `pub progress: Progress` (built in `handler.rs` from `context.meta.get_progress_token()` + `context.peer.clone()`). Hand-built calls get `Progress::disabled()`.
- Macro: `McpToolArg::Progress` recognised by type path last segment `Progress` (same heuristic as `CancelToken`). Allowed on tools, resources, prompts. Two `Progress` params ⇒ compile error.
- Capabilities: none to advertise (progress is base protocol).
- C4: under `json-response` the notification is silently dropped by rmcp? **Verify** during impl; if rmcp errors, `Progress` swallows it. Doc note either way.

### Tests (`tests/server/progress.rs`)

- token present ⇒ client receives N `notifications/progress` with the token, before the result, in order;
- no token ⇒ zero notifications;
- non-increasing report dropped;
- `json-response: true` ⇒ call succeeds, no notification, no error;
- compile-test: duplicate `Progress` param.

Size: S (~1 day).

**Shipped 2026-09-26.** `r2e-mcp/src/progress.rs`; `CallContext` in
`handler.rs` is the single construction site; `ToolCall::new` /
`ResourceCall::new` / `PromptCall::new` for hand-built calls. Open question 1
settled for progress: rmcp 3.1.4 `json_response` falls back to SSE when a
notification precedes the reply (`tower.rs` stateless path), so C4 does not
apply to progress — tested in `tests/server/progress.rs`.

---

## 2. Server→client requests — elicitation only

### 2a. Elicitation (first-class)

Enable rmcp feature `elicitation` (pulls `url`) behind an `r2e-mcp` feature
`elicitation` (on by default in `r2e`'s `mcp` feature — tiny dep).

**As shipped (deviations):** no feature gate — rmcp's `elicitation` feature
only adds `url`, which `r2e-mcp` already depends on, so it is enabled
unconditionally in the workspace rmcp dep. No per-`TypeId` schema cache (the
human round trip dominates; `ElicitationSchema::from_type` validates the
flat shape) and no `ElicitationSafe` (it would leak rmcp macros to users).
`elicit_url` takes an `elicitation_id` (spec-required). Config key is
`mcp.elicitation-timeout-secs` (+ `McpServer::with_elicitation_timeout`).
`ElicitError` also has `Cancelled` and `InvalidSchema`. Call structs carry a
`#[doc(hidden)] channel: ClientChannel` (peer + live flag + timeout +
cancel), not a raw `Option<Peer>`.

#### API

```rust,ignore
#[derive(Deserialize, JsonSchema)]
struct Confirm { confirmed: bool, reason: Option<String> }

#[tool(description = "Delete a project")]
async fn delete(&self, Params(p): Params<DeleteArgs>, client: McpClient) -> Result<String, McpError> {
    match client.elicit::<Confirm>("Really delete this project?").await? {
        Elicited::Accept(c) if c.confirmed => { self.repo.delete(p.id).await?; Ok("deleted".into()) }
        Elicited::Accept(_) | Elicited::Decline => Ok("kept".into()),
        Elicited::Cancel => Err(McpError::cancelled()),
    }
}
```

- `pub struct McpClient<'a>` — borrows the call (lifetime ties it to the member future → C2 enforced by the type system: can't be moved into `rt::spawn`).
  - `elicit<T: DeserializeOwned + JsonSchema>(message) -> Result<Elicited<T>, ElicitError>`; schema checked for flat-object shape (spec restriction: primitive properties only) **at first use, cached per `TypeId`** — boot-time check impossible since `T` is only known in the member body. Mirror rmcp's `ElicitationSafe` marker instead if it composes; decide during impl.
  - `elicit_url(message, url)` — URL-mode elicitation (OAuth-style out-of-band flows).
  - `supports_elicitation() -> bool` — from `peer.peer_info().capabilities.elicitation`.
  - `ElicitError`: `Unsupported` (client didn't advertise), `NoChannel` (json-response / stream gone), `Timeout`, `Transport`, `InvalidResponse` (didn't match `T`). `impl From<ElicitError> for McpError` → tool error result, not protocol error.
  - Timeout: `mcp.elicitation.timeout` (default 5 min), raced against the call's `CancelToken`.

#### Two execution models

| Peer | Mechanism |
|---|---|
| legacy session (< 2026-07-28, stateful) | direct `peer.create_elicitation(..)` on the POST's SSE stream; member future stays suspended. |
| 2026-07-28 (sessionless or not) | rmcp's request API already handles SEP-2260 routing for live requests; **but** a 2026 sessionless client may expect MRTR. Phase 2a ships live requests only; MRTR is 2c. |

Verify during impl which mode rmcp picks for a 2026 peer on a live
`create_elicitation` — if rmcp rejects live requests for sessionless 2026
peers, 2c becomes mandatory for 2026 clients and `elicit` returns
`ElicitError::Unsupported` until then.

#### Plumbing

- `ToolCall` etc. gain `peer: Option<Peer<RoleServer>>` (`#[doc(hidden)]` accessor); `McpToolArg::Client` resolves `McpClient<'_>` borrowing it. Lifetime param on a member arg is new for the macro — the generated closure must build it inside the async block (it already owns the call there).
- Visibility unaffected (outbound).
- Guards: none — elicitation is the member's own business.

#### Tests (`tests/server/elicitation.rs`)

A scripted rmcp client (`ClientHandler::create_elicitation`) returning accept /
decline / cancel / garbage; client without capability ⇒ `Unsupported`;
json-response ⇒ `NoChannel`; timeout; call cancellation while waiting;
compile-test: `McpClient` moved into `r2e_core::rt::spawn` fails to compile.

Size: M (2–3 days).

### 2b. Sampling + roots — out of scope

Deprecated by SEP-2577 (C1); not implemented. `McpClient` exposes no
`create_message`/`list_roots`, and no escape hatch to the raw peer is added
for them.

### 2c. MRTR (2026-07-28 input-required retries)

Only if 2a's verification shows 2026 peers need it, or once 2026 clients are
common. Design sketch, not committed:

- Member is re-run from scratch on retry (MRTR's model). `client.elicit::<T>(key, msg)` on retry returns the answer from `input_responses[key]`; on first pass it records an `InputRequest` and **aborts the member** via a sentinel `Err(ElicitError::InputRequired)` that the generated code maps to `CallToolResponse::InputRequired`.
- Side effects before `elicit` run twice → document: "elicit first, act after". Same contract as rmcp.
- `request_state`: sealed with rmcp `request-state` (`RequestStateCodec`, key from `mcp.request-state-key` secret, `${...}` supported) — carries the collected answers of earlier rounds; nothing server-side, so it works stateless and across replicas.
- Round cap: `DEFAULT_MRTR_MAX_ROUNDS` (10), configurable.
- Applies to tools, prompts and resource reads (rmcp has the three `*Response::InputRequired` variants).
- Only `InputRequest::Elicitation` is ever emitted — the `CreateMessage` / `ListRoots` variants are the deprecated sampling/roots (C1) and stay unused.

Size: L. Separate PR.

---

## 3. Completion — `completion/complete`

### API

```rust,ignore
#[mcp_routes]
impl Docs {
    #[prompt(description = "Summarise a project")]
    async fn summarise(&self, #[arg(complete = "project_names")] project: String) -> PromptResult { .. }

    #[resource(uri = "docs://{project}/{page}")]
    async fn page(&self, #[var(complete = "project_names")] project: String, page: String) -> ResourceResult { .. }

    #[completion]
    async fn project_names(&self, req: Completion<'_>) -> Vec<String> {
        self.repo.names_starting_with(req.value()).await
    }
}
```

- `#[completion]` member: `async fn(&self, req: Completion<'_>, <identity/request params as usual>) -> impl IntoCompletion` (`Vec<String>` or `CompletionInfo`-like `Completions { values, total, has_more }`).
- `Completion<'_>`: `value()`, `argument()` (name), `context_arguments()` (spec's already-filled args — lets `page` completion depend on `project`), `reference()` (prompt name / template URI).
- Wiring by name in the attribute (`complete = "fn_name"`), resolved **at macro time** within the same `#[mcp_routes]` impl ⇒ typo = compile error. Cross-impl sharing: the completion fn can be a plain method delegating to a bean.
- Exact attribute spelling depends on how prompt args / template vars are declared today (check `mcp_routes_parsing.rs` during impl; adapt, don't invent a second syntax).

### Handler

- Implement `ServerHandler::complete`:
  1. `prepare(&context)`.
  2. Resolve `r#ref` via `SessionView`: `Reference::Prompt(name)` → visible prompt; `Reference::Resource(uri_template)` → visible template by its **template string** (not an expanded URI). Hidden/unknown ⇒ `-32602 invalid params` identical for both (C6).
  3. Unknown argument / arg without provider ⇒ empty result (spec: completion is best-effort), not an error.
  4. Run the member's requirements check (same prologue as the prompt/resource it completes — reuse `requirements` of the **referenced** member, so completion can't leak values of a prompt the caller can't use).
  5. Truncate to 100 (`CompletionInfo::MAX_VALUES`), set `has_more` when truncated, keep provider's `total`.
- Route storage: `PromptRoute`/template routes gain `completions: Vec<(Cow<'static, str> /*arg*/, CompleteInvoke)>`; `SessionView` needs no change (lookup goes through the member).
- Capability: advertise `completions: {}` in `catalog.rs` capabilities **only if at least one provider exists** (boot-time, from the catalog). Dynamic/session-private members (`dynamic.rs`) accept providers too via the builder (`PromptRoute::with_completion(arg, closure)`).
- Cost: rate-limit is the app's job (it's an ordinary member → `#[intercept]`/`RateLimitGuard` apply). Doc it: completion fires per keystroke.

### Tests (`tests/server/completion.rs`)

prompt arg; template var with `context.arguments` dependency; 150 values ⇒
100 + `has_more`; hidden prompt ⇒ same error as unknown; requirement-failing
caller ⇒ same; no provider ⇒ empty; capability absent when no provider;
dynamic member with provider; compile-tests: unknown provider name, provider
with wrong signature, `complete` on a non-string arg.

Size: M (2–3 days, most in the macro).

---

## 4. Pagination — cursors on the four `*/list`

### Design

- Config `mcp.page-size` (default **none = unpaginated**, current behaviour; common values 50–200). Plugin builder `.page_size(n)`.
- Cursor = opaque base64url of `{ v: 1, off: u32, fp: u64 }`:
  - `off` — index into the **filtered** `visible_list` (stable ordering: catalog order is deterministic — verify it's not a `HashMap` iteration; if it is, sort by key once at build).
  - `fp` — fingerprint of the session view generation (`SessionView` is copy-on-write: add/stamp a `generation: u64` bumped on every mutation, plus a hash of the caller's principal subject). Mismatch ⇒ `-32602 "invalid cursor"` so the client restarts; `notifications/*/list_changed` already tells it to.
  - No crypto needed: a forged `off` only pages the caller's *own* visible list; `fp` includes subject so a cursor leaked across principals is rejected rather than silently reused. Document "cursors are not secrets".
- `ListXResult::with_all_items` → `ListXResult { items: page, next_cursor, .. }`.
- Tools/resources/templates/prompts share one `paginate(list, cursor, size, fp)` helper in `catalog.rs`.
- Performance: `visible_list` is O(n) per page → O(n²/size) for a full walk. Fine up to thousands; note it.

### Tests (`tests/server/pagination.rs`)

walk 3 pages = full list, no dupes; last page no `next_cursor`; garbage /
wrong-version cursor ⇒ -32602; view mutated mid-walk (group toggled) ⇒ -32602;
cursor from principal A replayed by B ⇒ -32602; `page-size` unset ⇒ single page
(regression guard for current clients).

Size: S–M (1–2 days).

---

## 5. Logging — out of scope

Deprecated by SEP-2577 (C1); not implemented. `logging/setLevel` keeps
rmcp's `method_not_found`, the `logging` capability is not advertised.
Document in `25-mcp.md`: server logs go to `tracing`/OTel; clients see
results (and progress messages), not server logs.

---

## 6. Order and PR slicing

| Order | Item | PR | Size | Breaking |
|---|---|---|---|---|
| 1 | Progress | P4-a | S | `ToolCall`/`ResourceCall`/`PromptCall` gain fields (public structs — breaking for hand-built calls; add constructors) |
| 2 | Elicitation live (2a) | P4-b | M | same structs gain `peer`; new default feature |
| 3 | Completion | P4-c | M | none (additive) |
| 4 | Pagination | P4-d | S–M | none while `page-size` unset |
| — | MRTR (2c) | after 2a verification | L | — |

Before P4-a: fold the three `*Call` construction sites in `handler.rs` into one
`CallContext` builder, so each later phase adds one field in one place.

Open questions to settle during impl (not user decisions — verify in rmcp):
1. rmcp behaviour of `notify_progress` / `create_elicitation` under `json_response = true`.
   **Answered:** rmcp switches the reply to SSE on the first notification /
   request. In session mode requests always get SSE, so `json_response` only
   matters stateless — where elicitation is impossible anyway (see 2).
2. Whether rmcp accepts a live `create_elicitation` toward a sessionless 2026-07-28 peer, or forces MRTR (decides whether 2c is required for 2026 clients).
   **Answered:** rmcp sends it (SEP-2260 association holds inside the
   handler), but only legacy session mode routes the client's answer POST
   back (`Mcp-Session-Id` → `accept_message`). Stateless and 2026
   per-request (`serve_negotiated_request_directly`, one-shot transport)
   accept the answer with 202 and drop it → the request would hang. So live
   elicitation is gated on `McpSession::is_persistent()` and fails fast with
   `NoChannel` elsewhere; **2c (MRTR) is required for 2026 sessionless
   clients.**
3. Catalog list ordering determinism (pagination prerequisite).
