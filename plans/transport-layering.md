# Roadmap — Transport layering: share more between `#[routes]`, `#[mcp_routes]` and `#[grpc_routes]`

Status: **levels 1–2 MERGED (#132, #133), 3a SHIPPED on PR #136 then RESCOPED 2026-11-04** — one type = one port, mixed form removed before release, 3b/3c dropped, level 4 (hexagonal DX) added; see §5 and the handoff in §10. Written after #1210
(struct-level identity on MCP services) showed that every identity feature
lands transport by transport. Follow-up to `docs/claude/transport-adapters.md`,
whose "wire-specific pieces, kept wire-specific" decision (§6) this plan
revisits for **one** item only: the identity contract. Nothing here changes
the public promise of `plans/axum-containment-roadmap.md` (R2E types, raw
axum only through `axum_compat`).

The question asked: *an app written against one transport should move to
another, or expose the same type on two, without rewriting the framework
side of its code.* Today it cannot. This plan says where the coupling is,
and proposes three levels of increasing ambition. Level 1 is pure
tech-debt; level 2 is a product change worth a core break; level 3 is a
product decision to take only when an app asks for it.

## 1. Where we actually are (measured 2026-11-03 on master, `r2e-macros/src`)

Three impl-side macros, **no edge between them**; each depends on the same
transport-neutral base:

| transport | parsing | codegen | imports from the shared base |
|---|---|---|---|
| HTTP `#[routes]` | `parsing/routes_parsing.rs` 911 | `codegen/{handlers 1859, controller_impl 1643, transverse 1092, wrapping 292, …}` | everything |
| MCP `#[mcp_routes]` | `parsing/mcp_routes_parsing.rs` 965 | `mcp_codegen/{mod 318, service_impl 555, tool_impl 472}` | `decorators.rs` ×4 fns, `handlers::qualified_controller_name`, `routes_parsing::reject_self_in_route_signature` |
| gRPC `#[grpc_routes]` | `parsing/grpc_routes_parsing.rs` 187 | `grpc_codegen/{mod 335, service_impl 178, trait_impl 252}` | `decorators.rs` ×4 fns, `handlers::qualified_controller_name` |

Shared base (no transport knowledge): `extract/` (attribute parsing into one
`MethodDecorators`), `parsing/identity_param.rs`, `parsing/controller_parsing.rs`,
`codegen/decorators.rs` (`spec_type_of`, `generate_named_deco_items`,
`wrap_with_interceptor_refs`, `endpoint_deps_fold`), and the struct side
`codegen/controller_codegen.rs`, whose generated items (`__r2e_meta_<Name>`,
`__R2eRequestData_<Name>`, `__R2eRequest_<Name>`, `bind_request`,
`guard_identity`, `request_identity`, `unscoped_request_data`) are the real
reuse channel: MCP references them a dozen times, gRPC reads one const
(`HAS_STRUCT_IDENTITY`, to refuse it).

What is written three times:

- the per-method parsing loop (collect decorators, identity parameter, doc,
  strip attributes) — `RouteMethod`/`SseMethod`/`WsMethod` (`model/types.rs`),
  `McpTool`, `GrpcMethod` all carry the same five fields
  (`name`, `decorators`, `identity_param`, `fn_item`, doc);
- the decorator storage layout — gRPC follows transport-adapters §3
  (`generate_named_deco_items` + `DecoSet`), MCP has its own `McpDecoLayout`
  over `decorator_product_field` (one shared field per controller site, one
  field per method site), HTTP has `CtrlDecoSet` + per-handler sets;
- the identity call path per member — `handlers.rs:838/938` and
  `tool_impl.rs:95-205` both walk the `__r2e_meta_` items by hand.

What ties an **application** to a transport (the user-facing part):

| coupling | HTTP | MCP | gRPC |
|---|---|---|---|
| member signature | `Json<T>`, `Path<T>`, `Query<T>`, `impl IntoHttpResponse` | `Params<T>`, `ToolCall`, `impl IntoToolResult` | prost `Request<T>` / `Response<T>` from the proto |
| identity contract | `FromRequestPartsVia<S, M>` over `http::request::Parts` | same (rides HTTP) | `GrpcIdentity` (a `DecoratorSpec`-backed trait over tonic `MetadataMap`, `r2e-grpc/src/identity.rs`) |
| struct-level identity / `#[inject(request)]` | yes | yes (#1210) | **no** (compile-time assert) |
| guards, interceptors, `#[roles]`, beans, config, `EndpointDeps` | shared | shared | shared |
| `GuardContext` builder | route envelope | `member_guard_context(Option<&Parts>)` — already has a *no parts* branch | `guard_context(&tonic::Request)` maps metadata → `HeaderMap`, `POST`, `/` |

So there are **two** identity contracts (`FromRequestPartsVia`, `GrpcIdentity`)
and **three** `GuardContext` builders. The member signature is the deepest
coupling and the only one with no cheap fix.

## 2. Level 1 — one member model, one decorator layout (tech debt, non-breaking)

**Status: DONE (2026-11-03, layout: `DecoSet` per member + one shared
controller set — `MemberDecoSets` — for MCP and gRPC; the flat
`McpDecoLayout` measured identical in the hotpath guard — 5 allocs/call,
same per-call decorator overhead — so the shared `DecoSet` layout won on
code size).** Task #1212, branch `task/transport-layering-l1`.

Exit-criteria outcome: suites green, no `.stderr` re-blessed (one new fail
case `mcp_empty_service_uninferable_guard`), same façade-binding block in
`cargo expand`. The line-count criterion was **not met**: `mcp_codegen` +
`grpc_codegen` went from 2110 to 1879 lines (−11 %, not −33 %). The remainder
is wire-specific by construction — MCP tool/resource/prompt argument
classification, completion wiring, JSON-schema emission; the tonic trait
impl and status projection on gRPC — and moving it to the shared layer would
only relocate it. The criterion is retired rather than chased.

Review follow-ups (codex, 2026-11-03) recorded here, not fixed in Level 1:

- Hidden decorator item names concatenate `<Ctrl>_<fn>` with `_`, so
  `A_b`/`c` and `A`/`b_c` collide. Pre-existing on HTTP, gRPC, pre-auth and
  scheduled sets (`generate_named_deco_items`); fix once for all kinds with
  a length-prefixed or hashed separator.
- HTTP decorator sets are still built by free functions, so `Self` in a
  `#[routes]` decorator expression does not resolve — only MCP/gRPC sets are
  owner-scoped (`impl <Ctrl> { fn __r2e_… }`). Scope the HTTP sets the same
  way when the HTTP emitter is touched next.

**Goal.** A new adapter is "a parser for its own attributes + an emitter";
the loop, the identity path and the decorator storage are written once.

**Scope.** `r2e-macros` only. No public API change, no version bump beyond the
patch the next release carries anyway. Generated code may change shape but
not behaviour; the compile-tests and the three suites are the oracle.

**Steps.**

1. `parsing/member.rs`: `MemberDef { name, fn_item, decorators, identity_param,
   doc, deprecated }` + `parse_member(&mut ImplItemFn, MemberRules)` where
   `MemberRules` carries the per-transport attribute allow-list (today
   `parse_decorators` / `parse_grpc_decorators` / `parse_mcp_decorators`
   differ only by that list — fold them into one function taking the list)
   and whether a struct-level identity is allowed. `RouteMethod`, `SseMethod`,
   `WsMethod`, `McpTool`, `GrpcMethod` embed `MemberDef` instead of repeating
   its fields. `reject_self_in_route_signature` moves there.
2. `codegen/decorators.rs`: retire `McpDecoLayout`; MCP uses
   `generate_named_deco_items` + `DecoSet` with kind `"McpDeco"` like gRPC.
   If the product layout (one field per site) is faster than `DecoSet`,
   measure first with the existing hotpath bench and keep the faster one —
   for all three.
3. `codegen/identity_path.rs`: one emitter for "bind the façade for this
   member" taking `(meta_mod, how_data_arrives)` where the arrival is an enum
   `{ ExtractHere /* HTTP: from parts in the handler */, FromDeposit /* MCP:
   request extensions */ }`. Both `handlers.rs:838` and `tool_impl.rs:95-130`
   call it. `guard_identity_expr` likewise.
4. `codegen/registration.rs`: one emitter for the `EndpointDeps` fold +
   witness assembly (`endpoint_deps_fold` + the marker tuple), used by the
   three `service_impl`/`controller_impl`.
5. Update `docs/claude/transport-adapters.md` §1–§4 so "what a new transport
   must provide" names these four entry points instead of "analogous to
   `grpc_codegen/`".

**Exit criteria.** `r2e-compile-tests`, `r2e-macros`, `r2e-mcp`, `r2e-grpc`,
`example-app` suites green with no `.stderr` re-bless except for paths that
moved; `mcp_codegen` + `grpc_codegen` lose at least a third of their lines;
`cargo expand` of one MCP service and one HTTP controller shows the same
façade-binding block.

**Size.** One PR, two sessions. Can be split in two PRs (steps 1–2, then 3–4).

## 3. Level 2 — one request head, one identity contract (product, core break)

**Status: DONE (2026-11-03).** Task #1213, branch `task/transport-layering-l2`
(stacked on L1).

Exit-criteria outcome: generated code builds every `GuardContext` through
`GuardContext::from_parts` (HTTP, MCP, gRPC; struct literals survive only in
tests). `grep GrpcIdentity` is not empty but only names the deprecated
empty stub (compile-time migration: its sealed supertrait carries a
`#[diagnostic::on_unimplemented]`, so an old impl fails with an E0277 naming
the migration page even under `#[allow(deprecated)]`, next to the "not a
member of trait" errors and the deprecation warning), the changelogs and
`docs/migration/grpc-identity.md`. example-grpc exposes
`AccountGreeter` (struct-level `AuthenticatedUser`, `#[roles]`, a
`#[request_helper]`), a server stream whose head is extracted before it
opens, and head/`ConnectInfo`/`Query` services. The missing-validator
trybuild case now fails at `register_grpc_service` through the
`GrpcService<S, W>` witness (stderr re-blessed). HTTP and MCP suites
unchanged. Deviations: item 4 uses `Arrival::ExtractHere` against the
wrapper's state (not `FromDeposit`); the URI comes from a
`CaptureMethodPath` routing layer (tonic trait methods do not see it) with
a `/<service>/<PascalCase(fn)>` fallback; the head deposits no
`MetadataMap` (an extractor builds `MetadataMap::from_headers(parts.headers.clone())`
itself) and inserts `ConnectInfo` only when none is there; HTTP guards keep
reading the peer address snapshotted before extraction; `#[anonymous]` stays disallowed on
gRPC (use `Option<I>`); the identity rejection is now the HTTP one
(`Unauthorized`, no `www-authenticate` — the `AuthenticatedUser` `HttpError`
rejection follow-up); r2e-core change is additive (`ModuleEndpoints<T, W>`
witness), no core break. The transport-adapters §6 reversal is in its
Decisions log.

**Goal.** `#[inject(identity)]`, `#[inject(request)]`, struct-level identity and
every `FromRequestPartsVia` extractor work on **every** transport the same
way; a `GuardContext` is built by one function. Moving a service from HTTP
to MCP to gRPC changes nothing in its identity or guard declarations.

**Design — pick `Parts` as the universal head, not a new trait.**
`http::request::Parts` is an `http`-crate type (not axum), already the head
for HTTP and MCP, and the thing axum-ecosystem extractors (`HeaderMap`,
`TypedHeader`, extensions) need. `r2e-mcp` already synthesizes one when there
is no transport (`call_parts(None)`), and `r2e-grpc` already views
`MetadataMap` as a `HeaderMap` for guards. So:

1. `r2e-core`: `GuardContext::from_parts(parts, method_name, controller_name,
   identity, path_params)` — the one builder. The three existing builders
   become thin calls to it (MCP's `None` branch synthesizes empty `Parts`).
2. `r2e-grpc`: build `Parts` per call from the tonic request (method `POST`,
   uri `/<service>/<method>` — a real improvement over today's `/` for
   guards and logs —, headers = metadata view, extensions moved over,
   `ConnectInfo` from `remote_addr`). Identity extraction then goes through
   `FromRequestPartsVia<S, M>` with the state, exactly as MCP does: the
   validator is found as a bean (`ViaBean`) and compile-checked at
   `register_grpc_service` through the witness `W` of a new
   `GrpcService<S, W>` — same shape as `McpService<S, W>` (#1175).
3. **Remove `GrpcIdentity`.** `AuthenticatedUser` already implements the HTTP
   contract; the `grpc` feature of `r2e-security` keeps only what differs
   (the `authorization` metadata lookup is the same header). An identity
   type that needs raw tonic metadata reads it from the `Parts` extensions
   (we insert the `MetadataMap` there).
4. `#[grpc_routes]` drops its `HAS_STRUCT_IDENTITY` assert and gains the
   level-1 `FromDeposit` arrival (one extraction per call in the tonic
   service wrapper, before dispatch) — struct-level identity and
   `#[inject(request)]` on gRPC for free.
5. Streaming members (`SseMethod`, `WsMethod`, gRPC server streams) follow the
   same path; the head is extracted before the stream opens.

**Breaking.** `r2e-grpc` (`GrpcIdentity` removed, `GrpcService<S, W>`
witness), `r2e-security` `grpc` feature, `r2e-macros` (codegen contract
with `r2e-grpc`), propagated to the façade. `r2e-core` gets
`GuardContext::from_parts` (additive) unless `GuardContext` fields change
(they should not). Migration page `docs/migration/grpc-identity.md`:
"implement `FromRequestPartsVia` (or axum's `FromRequestParts`) instead of
`GrpcIdentity`; the validator becomes a bean". Compile-time path: the old
trait is gone, the error names the page.

**Why not a `RequestHead` trait.** It would be `Parts` with a worse name, and
every axum-shaped extractor would need a bridge. The only thing `Parts`
costs a non-HTTP transport is synthesizing a method and a URI, which gRPC
and MCP already do.

**Decision needed before starting.** This overturns transport-adapters §6
first bullet ("identity extraction — wire-specific only where the wire has
no HTTP request head"). Record the reversal in the Decisions log when
acknowledged. Batch it with the next `r2e-core` break if any is pending
(see `feedback_version_propagation_stays`).

**Exit criteria.** One `GuardContext` builder in the workspace; `grep
GrpcIdentity` empty; example-grpc exposes a service with a struct-level
`AuthenticatedUser` and a `#[roles]` member; a trybuild case proves a
missing validator bean fails at `register_grpc_service`; the HTTP and MCP
suites unchanged.

**Size.** Two PRs (core + grpc, then macros + docs + migration), three to four
sessions, plus one release PR.

## 4. Level 3 (as planned 2026-11-03) — SUPERSEDED 2026-11-04, kept as record

**Status: 3a DONE (Tasker #1215, PR #136, 2026-11-03) and then rescoped: the
mixed HTTP + MCP form is removed before release; 3b and 3c are dropped.** The
decision and the new levels are in §5–§7; the 3a handoff and record below are
kept as written because the release recipe and the shared-code map still hold.

**Status: 3a DONE (Tasker #1215, branch `task/transport-layering-l3a`,
2026-11-03); 3b NEXT after its own design round; 3c only if the explicit
registration pair reads badly in a real app.** The user decided level 3 goes
ahead on 2026-11-03. The handoff block below is kept as written; the 3a
record follows it.

- *3a as shipped.* `#[routes]` accepts `#[tool]`/`#[resource]`/`#[prompt]`/
  `#[completion]` members and `#[routes(group, opt_in)]`. A dispatcher
  (`r2e-macros/src/attrs/routes_attr.rs`) classifies the impl: HTTP-only and
  MCP-only go through their existing parser + codegen untouched (expand
  diff: byte-identical on every non-reworked target, except the MCP core-store
  contract of review fix round 1 — new `construct` fn, `__core: Arc<Self>`
  on `routes`, `core: __core`); a mixed impl is split
  by member, each half parsed by its own parser, then the HTTP codegen emits
  the one façade impl and the one `EndpointDeps` (HTTP + MCP sites), the MCP
  codegen skips its carrier (`McpRoutesImplDef::mixed_http_sites`), and
  `validate_config` unions both site sets. Each trait keeps its witness.
  Impl-level `#[guard]`/`#[roles]`/`#[all_roles]`/`#[intercept]` apply to
  both transports; their decorators are built once per transport, but both
  registrations share ONE core per type and builder (review fix round 1:
  core store `endpoint_core_or_insert` in the builder, `McpService::construct`
  + `routes(state, core, ctx)`, lifecycle/background members require
  `register_controller` — checked at `build()`); `#[pre_guard]`/`#[middleware]`/
  `#[layer]` stay HTTP-only (rejected with no HTTP route). `#[get]`+`#[tool]`
  on one method and an MCP marker in `#[grpc_routes]` are compile errors.
  `#[mcp_routes]` = `#[deprecated(since = "0.10.0")]` alias through the same
  dispatcher (dropped by the 3a' rescope, see §5). `example-mcp`'s `MathTools`
  serves an HTTP route and its tools from one impl.
- *Learnings.* (1) Split-and-reuse beat merging the parsers: it is what made
  byte stability free. (2) A proc-macro `#[deprecated]` fires at the
  attribute path — an item-level `#[allow(deprecated)]` on the impl does NOT
  silence it; a module/crate-level `#![allow(deprecated)]` does. (3) An
  empty impl reads as HTTP under `#[routes]`, so an empty MCP service is only
  spellable through the alias (one compile-fail case stays on it).
  (4) `scripts/smoke-registry.sh` kept `#[mcp_routes]` until the alias was
  dropped (it builds against the published crates, which predate MCP
  members in `#[routes]`; `--patch` is the pre-release check).

- *Prerequisites.* #132 (level 1) merged; #133 (level 2) merged; release PR
  cut with `scripts/bump-version.sh` (`r2e-grpc breaking`, `r2e-security
  breaking`, `r2e-macros breaking --no-propagate` + patch re-pins on
  `r2e-core`/`r2e-executor`/`r2e-openapi`/`r2e`; façade `r2e` 0.11.0);
  `scripts/smoke-registry.sh` green after it. Check `git log master` and
  the crates' `CHANGELOG.md` before starting — do not assume.
- *Method.* As #1212/#1213: one Tasker ticket (label `macros`, `breaking`
  for 3a), one branch off `master`, implementation by an Opus subagent,
  full verification (r2e-macros, r2e-compile-tests, r2e-mcp, r2e-grpc,
  example-app/-mcp/-grpc, llm-doctests, `clippy --workspace --tests -D
  warnings`, `check-versions`/`check-dep-boundary`/`check-source-boundary`/
  `check-llm-docs`), codex gpt-6-astra review loop to `VERDICT: MERGE`.
  Never `cargo fmt --all`; no version change in the feature PR.
- *Where the shared pieces live (after levels 1–2).* `r2e-macros/src/parsing/
  member.rs` (one `MemberDef`/`parse_member`/`MemberRules`),
  `codegen/decorators.rs` (`MemberDecoSets`), `codegen/identity_path.rs`
  (`bind_facade`, `Arrival`, `guard_identity`), `codegen/registration.rs`
  (`DepsFold`, `Witness`), `r2e-core/src/decorators/guards.rs`
  (`GuardContext::from_parts`), `r2e-grpc/src/head.rs`. The three impl
  macros still have separate codegen roots (`codegen/handlers.rs`,
  `mcp_codegen/`, `grpc_codegen/`): 3a merges the HTTP and MCP ones.
- *The blocker 3a removes.* One type = one `EndpointDeps` impl
  (`docs/claude/transport-adapters.md` §4): the merged macro folds one
  `EndpointDeps` over every HTTP and MCP site and emits both
  `Controller<S, W>` and `McpService<S, W>`. Registration stays explicit
  per transport. `#[mcp_routes]` becomes an alias of the merged macro for
  one release, then is removed (façade minor, `Breaking` entry in
  `r2e-macros`/`r2e-mcp`/`r2e` changelogs, migration page if more than a
  rename).
- *Design notes carried from level 2.* (1) `#[grpc_routes]` rewrites
  `Self::<Assoc>` in moved method bodies (`parsing/grpc_routes_parsing.rs`,
  `substitute_self_assoc`); it took three review passes to cover and has
  documented residual limits. If 3a re-homes gRPC methods too, keep user
  methods inside a macro-generated trait impl carrying the associated types
  so the compiler resolves `Self::X` — do not extend the rewrite.
  (2) Every state-generic registration trait carries its own inference-only
  witness `W` (`Controller`, `McpService`, `GrpcService`) and every fold
  layer threads it (`ModuleControllers<T, WC>`, `ModuleEndpoints<T, WE>`,
  `ModuleList` `((WC, WE), WR)`); a fourth transport adds a channel. 3c is
  the point to collapse them — not before. (3) Tasker #1214 (one
  config-error report formatter) is grouped with 3a: the first adapter that
  would otherwise copy the banner uses the shared formatter instead.
- *Follow-ups carried, not blocking 3a.* `GrpcIdentity` deprecated stub:
  remove at the `r2e-grpc` release after the one that ships #1213
  (`Breaking` entry, migration page already exists). `AuthenticatedUser`'s
  rejection is `HttpError`, so a validator failure drops `www-authenticate`
  on HTTP and on gRPC (metadata). `#[anonymous]` stays disallowed on gRPC
  (`Option<I>` instead). Level-1 review follow-ups in §2 (hidden decorator
  item-name collision; `Self` in HTTP decorator expressions). `#[mcp_routes]`
  deliberate drifts in `r2e-macros/CHANGELOG.md` (#1212). Wire the builder's
  per-type core store (`endpoint_core_or_insert`) to `register_grpc_service`
  next, so a type served on HTTP and gRPC shares one core as HTTP + MCP do
  (#1215 fix round 1 left gRPC constructing its own). Pre-state
  `register_controller` (hand-written impls on `AppBuilder<NoState>`) is
  silently discarded at `build_state()` — pre-existing; the per-type core
  store is reset with it (#1215 fix round 3); consider a compile-time
  rejection.
- *Exit criteria for 3a (to confirm in the ticket).* One impl carrying
  `#[get]`/`#[post]` and `#[tool]`/`#[resource]`/`#[prompt]` members
  compiles and registers on both transports with one identity/guard
  declaration; HTTP-only and MCP-only impls keep their generated items
  byte-for-byte (`cargo expand` diff); `#[mcp_routes]` alias emits a
  deprecation; one example (`example-mcp` or a blumana-survey-shaped
  service) exposes the same operation on both; compile tests for a member
  kind mixed into the wrong attribute; `llm/*.md` + book + changelogs
  updated; no existing `.stderr` re-blessed without a stated reason.

**Goal.** The thing the user actually asked for: one type, one member body,
exposed on HTTP and MCP (and, within limits, gRPC) without rewriting it, and
a transport switch that is a change of attribute, not of signature.

**What blocks it today.**

- *One type = one `EndpointDeps` impl* (transport-adapters §4): a struct
  cannot carry `#[routes]` and `#[mcp_routes]` at once.
- Member signatures are the wire's types (§1 table). A neutral member needs
  a neutral input/output contract and a projection per wire.
- gRPC's contract is the proto: prost messages are generated outside Rust.
  A Rust-first operation cannot become a tonic service without a proto. The
  only projection that exists in the ecosystem is Connect-style JSON over
  HTTP, which is HTTP again. **gRPC stays an explicit adapter at this level;
  the plan does not pretend otherwise.**

**Shape (to decide, not to build yet).**

- 3a — *multi-kind types.* Merge `#[routes]` and `#[mcp_routes]` into one
  impl macro that accepts `#[get]`/`#[post]`/… and `#[tool]`/`#[resource]`/
  `#[prompt]` members on the same impl, folds one `EndpointDeps` over every
  site, and emits both `Controller<S, W>` and `McpService<S, W>` when both
  kinds are present. Registration stays explicit per transport
  (`register_controller` + `register_mcp_service` on the same type) so the
  builder never has to know `r2e-mcp`. Level 1 makes this a parsing change;
  level 2 makes the identity identical on both. Breaking: `#[mcp_routes]`
  becomes an alias then is removed (façade minor).
- 3b — *neutral signature + projections.* A member written
  `async fn create(&self, input: CreateSurvey) -> Result<Survey, AppError>`
  with `#[expose(http = post("/surveys"), mcp = tool("create_survey"))]`.
  Projection rules: HTTP = JSON body in / JSON out with `ApiError`
  projection; MCP = `Params<CreateSurvey>` schema in / `IntoToolResult` out
  with `McpError` projection. Path params, headers and streaming have no
  neutral form and stay on explicit per-wire members. Needs
  `RequestBodySchema` / `ResponseBodySchema` (`r2e-core/src/di/meta.rs`, behind `openapi`) for the MCP
  schema. This is a new member kind, additive, no break.
- 3c — *single registration.* `register_endpoint::<T>()` dispatching to every
  registry `T` declares. Only worth it after 3a exists and if the explicit
  pair reads badly in real apps. Not before.

**Trigger.** An app that needs the same operations on HTTP and MCP (the
blumana-survey MCP endpoint duplicating the REST surface would be the
candidate). Until then: roadmap only.

**Size.** 3a: one PR after levels 1–2, two sessions. 3b: a design round
first (`plans/`), then two PRs. 3c: not estimated.

## 5. Decision 2026-11-04 — one type = one port; unify the generator, not the syntax

Taken after 3a shipped on PR #136 and the call/dependency trees were compared
(old vs new) in the review session. The user's end goal is **hexagonal**:
every inbound port is its own adapter over the same beans —

```
http    → #[controller] + #[routes]      → #[inject] services
mcp     → #[controller] + #[routes]      → #[inject] services   (its own type)
grpc    → #[controller] + #[grpc_routes] → #[inject] services
events  → #[consumer] on a bean          → #[inject] services
```

— with Rust's compile-time safety end to end (DI, identity, roles, guard
deps, error projection) and the least code per adapter. "Unify" meant **the
code behind port generation**, not letting one type mix ports.

- **One type = one port.** Mixing `#[tool]`/`#[resource]`/`#[prompt]`/
  `#[completion]` with HTTP route markers in one impl is a **compile error**
  that names the hexagonal page. The mixed HTTP + MCP form shipped by 3a is
  **removed before release** (same branch, same PR, squash-merged: it never
  reaches a changelog).
- **3b dropped.** A neutral member signature contradicts the doctrine: each
  port has its native types by construction (`Json`/`Path` vs
  `Params`/`ToolCall` vs prost), and those types carry schema, content type
  and streaming semantics that a neutral form would erase.
- **3c dropped.** Single registration only made sense for multi-port types.
- **No deprecated alias (user decision 2026-11-04).** `#[mcp_routes]` is
  removed outright with a migration note in the changelogs (rename to
  `#[routes]`): R2E apps are mostly generated by agents, which read the
  current docs, so an alias is code to maintain for nobody. An MCP service
  with no member is therefore not expressible (an empty impl reads as HTTP).
- **Kept from 3a** (value independent of mixing): `#[routes]` infers the port
  from its members (`#[mcp_routes]` removed, see above);
  the builder's per-type core store (`endpoint_core_or_insert`) — a
  controller mounted twice (prefixed modules) shares one core and one
  lifecycle; `McpService::{construct, routes(state, core, ctx)}` — the same
  shape as `Controller`, which is what 3b' builds on; the identity path and
  `DepsFold`; #1214.
- **Cost of the mixed form, measured on the branch, for the record:** 62
  occurrences of mixed plumbing (`MixedMcp`, `mixed_http_sites`,
  `facade_methods`, `site_exprs`, `controller_hosted_markers`, the three
  marker-classification lists) across 11 `r2e-macros` files; 15
  occurrences of the build-time fail-fast (`NEEDS_CONTROLLER_REGISTRATION`,
  `require_controller_registration`, `check_controller_registrations`)
  across 6 files in `r2e-core`/`r2e-mcp`/`r2e-macros`; `routes_attr.rs` 217
  lines where ~40 do the port dispatch; seven dedicated tests; `example-mcp`
  as showcase. Per-call cost nil. The recurring cost was the three
  classification lists (every new `#[routes]` attribute had to be filed),
  one `EndpointDeps` carrier spanning both ports (inference errors reported
  on every site), a runtime check that existed only because registration is
  split across two crates, decorator instances per registration on a shared
  core, and a showcase that contradicted the doctrine.

## 6. Level 3 rewritten — one port per type, one generation pipeline

**3a' — remove the mixed form** (pre-release, on `task/transport-layering-l3a`,
Tasker #1215 rescoped). Handoff in §10.

**3b' — parse once, one pipeline** (Tasker #1216; internal tech debt,
non-breaking, after the release). The generator is unified at the code level: one
`Vec<MemberDef>` per impl (already parsed by `parsing/member.rs`), one
codegen pipeline driven by a port description inside `r2e-macros` (rules,
member emission, service-impl emission), and three implementations —
HTTP, MCP, gRPC. gRPC joins the pipeline **keeping `#[grpc_routes]` as its
attribute** (the proto decides the port; nothing to infer) and keeps user
methods inside a macro-generated trait impl carrying the associated types,
which retires the `Self::<Assoc>` rewrite (`substitute_self_assoc`). The
dispatcher in `attrs/routes_attr.rs` shrinks to port detection + the
mixed-kinds error. Exit criteria: `cargo expand` byte-identical on every
HTTP and MCP target; no `.stderr` re-blessed without a stated reason; the
three `*_routes_parsing.rs` files no longer own a member loop. Measured
starting point (branch, lines): `routes_parsing` 835, `mcp_routes_parsing`
927, `grpc_routes_parsing` 529, `handlers` 1862, `mcp_codegen/*` 1262,
`grpc_codegen/*` 802. Evaluate, without promising, whether the per-trait
witnesses `W` can be collapsed once the pipeline is single.

**3c' — runtime hygiene** (Tasker #1217; the carried follow-ups, grouped;
patch or `Changed` entries unless noted): wire the per-type core store to
`register_grpc_service`; reject pre-state `register_controller` at compile
time instead of discarding it silently at `build_state()`; restore
`www-authenticate` on validator failure (HTTP and gRPC metadata —
`AuthenticatedUser`'s rejection is `HttpError`); hidden decorator item
names `<Ctrl>_<fn>` can collide across modules; `Self` in HTTP decorator
expressions; remove the `GrpcIdentity` stub at the minor after the release
(the `#[mcp_routes]` alias was never shipped: removed outright on #136).

## 7. Level 4 — hexagonal DX (the end goal)

Mostly projection, one example and documentation; no macro work.

1. **One domain error projects to every port** (Tasker #1218). `#[derive(ApiError)]` already
   projects to HTTP and, via `From<HttpError> for McpError` /
   `From<Rejection> for McpError`, to MCP. gRPC has `rejection_to_status`
   only: add `From<HttpError> for tonic::Status` (status mapping + message
   policy as `rejection_to_status`), so a service's `Result<T, AppError>`
   compiles on all three adapters without glue.
2. **Reference example + `llm/hexagonal.md`** (Tasker #1219, with item 4). `example-app` restructured as
   `domain/`, `ports/`, `adapters/{http,mcp,grpc,events}`; one llm topic
   routed from `llm.txt` (slug added to `TOPICS` in
   `r2e-cli/src/commands/llm_docs.rs`) and one book chapter. Outbound ports
   as traits: `Arc<dyn Port>` provided as a bean, swapped with
   `override_bean` in tests — documented, nothing to build.
3. **Transport-leak guard** (Tasker #1220). An `r2e doctor` rule (heuristic, say so) that
   flags `r2e::http`/`Json`/`ToolCall`/`tonic` imports under `domain/` and
   `services/`. The one point of the doctrine the compiler cannot hold.
4. **Inbound adapter catalogue** (in #1219). HTTP, MCP, gRPC, `#[consumer]`,
   `#[scheduled]`: what each injects (identity, request scope, config) and
   what it projects (errors, responses). A DB queue / outbox adapter is a
   separate ticket, only on a real need.

## 8. Order and dependencies

```
3a' remove mixed  →  ONE release (L1 + L2 + 3a' + #1214)  →  3b' pipeline  →  3c' hygiene
                                                            ↘  level 4 (independent of 3b'; may start right after the release)
```

The release is the one planned in §4's handoff (r2e-grpc / r2e-security /
r2e-mcp breaking; r2e-macros breaking `--no-propagate` + patch re-pins;
façade r2e 0.11.0). Nothing from 3b', 3c' or level 4 waits on it except the
`GrpcIdentity` stub removal, which needs the deprecation window (the
`#[mcp_routes]` alias was never shipped: removed outright on #136, §5).

## 9. Not in this plan

- A neutral member signature across ports (3b) and a single
  `register_endpoint` (3c): dropped 2026-11-04, see §5.
- One type exposed on two ports: removed 2026-11-04. Two types over the same
  beans is the supported shape.
- A second HTTP backend or a `Router` newtype (`plans/axum-containment-roadmap.md` §5.3c stays deferred).
- Per-transport guard traits (resolved the other way 2026-08-27, see transport-adapters invariants).
- Generating protos from Rust types for gRPC.
- The `#[scheduled]` adapter: timer-driven, no request head, no identity; untouched.
- A DB-queue / outbox inbound adapter without a concrete request.

## 10. Handoff — 3a' then the release (fresh session)

Read this section, `docs/claude/transport-adapters.md`,
`docs/claude/testing-conventions.md` and the memory file
`project_transport_layering.md` first. Check `git log master..task/transport-layering-l3a`
and the four `[Unreleased]` sections before touching anything — do not
assume the branch is where this text says.

**Ground rules (unchanged from #1212/#1213/#1215).** One ticket (#1215,
rescoped), the existing branch `task/transport-layering-l3a` off `master`,
**no worktree**, implementation by an Opus subagent, builds serialized
(shared target-dir), **never `cargo fmt --all`**, no version change in the
feature PR, no Claude trailers on commits and no footer on the PR, the user
merges (squash). Full gate: `cargo test -p r2e-macros -p r2e-compile-tests
-p r2e-mcp -p r2e-grpc -p r2e-core`, `example-app`/`example-mcp`/
`example-mcp-oauth`/`example-grpc`, `llm-doctests`, `cargo clippy --workspace
--tests -D warnings`, `scripts/check-versions.sh`, `check-dep-boundary.sh`,
`check-source-boundary.sh`, `check-llm-docs.sh --update`; then the codex
loop (`codex exec -m gpt-6-astra --sandbox workspace-write
--skip-git-repo-check "$(cat prompt)" < /dev/null`, unsandboxed, background,
verify the `model:` header) to `VERDICT: MERGE`.

**Step 1 — remove the mixed form (macros).**

- `r2e-macros/src/attrs/routes_attr.rs`: the mixed branch becomes a compile
  error on the first offending member: *"`#[tool]`, `#[resource]`,
  `#[prompt]` and `#[completion]` cannot share an impl with HTTP routes: one
  type exposes one port. Move the MCP members to their own `#[controller]`
  type over the same beans (docs/features/25-mcp.md § One port per type)."*
  Keep: HTTP-only → `routes_parsing`, MCP-only → `mcp_routes_parsing`, the
  "empty impl reads as HTTP" rule. `#[mcp_routes]` is removed outright
  (user decision, §5): an empty MCP service is no longer spellable — accepted.
- Delete `MixedMcp`, `mixed_http_sites`, `facade_methods`, `site_exprs`,
  `controller_hosted_markers`, `HTTP_SIDE_MARKERS`,
  `CONTROLLER_HOSTED_MARKERS` and every branch keyed on them (11 files:
  `attrs/routes_attr.rs`, `codegen/{controller_impl,decorators,registration,wrapping}.rs`,
  `mcp_codegen/{mod,service_impl}.rs`, `grpc_codegen/{mod,service_impl}.rs`,
  `parsing/{routes_parsing,mcp_routes_parsing}.rs`). Keep whatever
  `HTTP_ONLY_IMPL_DECORATORS` still serves: an HTTP-only impl decorator
  (`#[pre_guard]`/`#[middleware]`/`#[layer]`) on an MCP impl must still be
  rejected (`mcp_impl_middleware_not_supported` stays).
- Delete `McpService::NEEDS_CONTROLLER_REGISTRATION`,
  `AppBuilder::require_controller_registration`,
  `EndpointCores::needs_controller`, `check_controller_registrations`
  (`r2e-core/src/builder/{mod,typed}.rs`, `r2e-mcp/src/{lib,service}.rs`,
  `r2e-macros/src/mcp_codegen/service_impl.rs`). Keep
  `EndpointCores::{cores, controllers}`, `endpoint_core_or_insert`, the
  `from_pre` store reset, and `McpService::{construct, routes(state, core, ctx)}`
  (decision: same shape as `Controller`; 3b' relies on one shape per port).
- Expand-diff gate: HTTP-only and MCP-only targets byte-identical to the
  current branch head.

**Step 2 — tests.** Delete the HTTP + MCP cases of
`r2e-mcp/tests/server/single_core.rs` (keep the HTTP twice-mounted ones —
they pin the shared-core `Changed` entry; move them to `r2e-core/tests/` if
they no longer need `r2e-mcp`, and drop the `r2e-events` dev-dep if nothing
else uses it). Compile tests: delete `mcp_mixed_missing_bean`,
`mcp_mixed_missing_guard_dep`, `mcp_tool_anonymous_mixed`, pass cases
`mcp_mixed_impl_middleware` and `mcp_mixed_struct_identity`; replace
`mcp_member_also_http_route` by `mcp_member_in_http_impl` (a `#[tool]` next
to a `#[get]` member) pinning the new error text; check
`mcp_uninferable_every_site` and `grpc_mcp_member_rejected` still mean what
they say. Add nothing else: the MCP-only and HTTP-only suites already cover
the kept behaviour.

**Step 3 — examples.** `examples/example-mcp/src/app.rs` back to two
adapters over one service: `CalcController` (`#[controller(path =
"/api/calc")] #[routes]`, HTTP) and `MathTools` (`#[controller] #[routes]`,
`#[tool]`s only) both injecting `CalcService`; `ScientificTools` unchanged.
This is the hexagonal shape in miniature — say so in the file header.
Check `examples/example-mcp-oauth` for the same pattern.

**Step 4 — docs, so the mixed form never existed.**
`docs/features/25-mcp.md` (intro sentence, the "one impl may mix" clause;
the mixed section around the second `MathTools` listing becomes **"One port
per type"**: the compile error, the two-types shape, why);
`docs/claude/transport-adapters.md` (mentions of `mixed_http_sites` and
"a mixed impl is both a `Controller` and an `McpService`");
`docs/claude/guards-interceptors.md` (decorators "once per registration":
keep the per-mount HTTP statement, drop the MCP clause);
`r2e-macros/src/lib.rs` rustdoc of `#[routes]` and `#[mcp_routes]`;
`llm/mcp-server.md`, `llm/interceptors.md` (the "MCP members of a mixed
impl" wording), `llm/grpc.md` if it leans on the mixed form, then
`scripts/check-llm-docs.sh --update` and commit `llm-full.txt`.
`llm/core-concepts.md` "mixed public/protected controllers" is about
parameter identity — leave it.

**Step 5 — changelogs, rewritten under `[Unreleased]` (no "added then
removed" trace).** `r2e-macros` Breaking #1215: `#[routes]` accepts MCP
members, infers the port, `#[mcp_routes]` removed with a migration note,
MCP markers next to HTTP routes are a compile error. `r2e-mcp` Breaking: services declared
with `#[routes]`; `McpService::{construct, routes(state, core, ctx)}`;
`Send + Sync`; members run on the façade (`Self` rules); witness starts with
`((),)`; **drop** the shared-core-with-HTTP and fail-fast entries.
`r2e-core`: keep Added `endpoint_core_or_insert` and the `Changed` entry for
a twice-registered controller (HTTP-only fact; Changed vs Breaking stays
the user's call at release); **drop** `require_controller_registration`.
`r2e-grpc` untouched. Then update Tasker #1215 (description + note) and
rewrite PR #136's title and body (no footer).

**Step 6 — the release**, once the user has squash-merged #136. Levels 1–2
were deliberately left unreleased so that 3a' ships in the same release
(one breaking cycle on r2e-macros / r2e-mcp / r2e instead of two — user
decision 2026-11-03; the docs-only handoff PR #135 that recorded it was
closed unmerged, superseded by this section). The recipe was tried on
2026-11-03 as release PR #134, closed unmerged on purpose (branch
`release/r2e-0.11.0` kept): propagated `--dry-run` first, then
`scripts/bump-version.sh --no-propagate` **unsandboxed** with the explicit
list — `r2e-grpc`, `r2e-security`, `r2e-mcp` breaking; `r2e-macros` breaking
`--no-propagate` + patch re-pins `r2e-core`/`r2e-executor`/`r2e-openapi`;
`r2e-oidc`/`r2e-test` carried with hand-written `[Unreleased]` entries
(wording in commit `5e66ec7d`); façade `r2e` 0.11.0 + `r2e-cli`. With
`--no-propagate` every carried crate is listed explicitly and counts as
*requested*, and the script refuses a requested crate with an empty
`[Unreleased]`: write the hand-written entries (r2e-oidc, r2e-test, and the
"Re-pins `r2e-macros` …" lines of r2e-core / r2e-executor / r2e-openapi)
before running it. `scripts/smoke-registry.sh` already spells `#[routes]`:
its registry mode only passes once the release is published. Open
`release: …` (no footer); the user merges; run
`scripts/smoke-registry.sh`. Record the outcome in
`project_release_pipeline` and `project_transport_layering`.

**Then** 3b' (#1216), 3c' (#1217) and level 4 (#1218 → #1219 → #1220) are
their own tickets and branches — not on this branch.
