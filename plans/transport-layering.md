# Roadmap — Transport layering: share more between `#[routes]`, `#[mcp_routes]` and `#[grpc_routes]`

Status: **PROPOSED** 2026-11-03, not yet acknowledged. Written after #1210
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

## 4. Level 3 — neutral operations exposed on several transports (product decision)

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
  dispatcher, removed next minor. `example-mcp`'s `MathTools` serves an HTTP
  route and its tools from one impl.
- *Learnings.* (1) Split-and-reuse beat merging the parsers: it is what made
  byte stability free. (2) A proc-macro `#[deprecated]` fires at the
  attribute path — an item-level `#[allow(deprecated)]` on the impl does NOT
  silence it; a module/crate-level `#![allow(deprecated)]` does. (3) An
  empty impl reads as HTTP under `#[routes]`, so an empty MCP service is only
  spellable through the alias (one compile-fail case stays on it).
  (4) `scripts/smoke-registry.sh` keeps `#[mcp_routes]` until the release:
  it builds against the published crates, which predate MCP members in
  `#[routes]`.

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
  (#1215 fix round 1 left gRPC constructing its own).
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

## 5. Order and dependencies

```
Level 1 (macros tech debt)  →  Level 2 (one head, one identity contract)  →  3a  →  3b  →  3c
      non-breaking                 grpc/security/macros minor, core patch      façade minor   additive
```

Level 1 is worth doing alone. Level 2 depends on level 1 step 3 (the shared
identity emitter is what gRPC plugs into). 3a depends on both. 3b depends
on 3a only for the single-impl ergonomics; it could be prototyped on
`#[routes]` + `#[mcp_routes]` separately but would then duplicate the
projection attribute parsing — don't.

## 6. Not in this plan

- A second HTTP backend or a `Router` newtype (`plans/axum-containment-roadmap.md` §5.3c stays deferred).
- Per-transport guard traits (resolved the other way 2026-08-27, see transport-adapters invariants).
- Generating protos from Rust types for gRPC.
- The `#[scheduled]` adapter: timer-driven, no request head, no identity; untouched.
