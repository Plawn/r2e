# Transport Adapters — Adding a Wire Format to R2E

Status: **REFERENCE** — recorded 2026-07-09, verified against the code
2026-07-23; MCP added as the third wire adapter 2026-08-27. Read this before
wiring a new entry point (a wire protocol, a message-driven dispatcher,
anything that invokes endpoint methods from outside). Nothing here is pending
work: every mechanism described is implemented.

## The architecture in one paragraph

R2E is ports-and-adapters shaped. The **middle layer is transport-neutral
and already built**: the bean graph (`BeanContext`, `ContextConstruct`),
the decorator machinery (`DecoratorSpec`, `Interceptor<R>`,
`build_decorator`), and the compile-time dep checking (`EndpointDeps` +
`AllSatisfied`). A transport adapter supplies only what is genuinely
wire-specific: request extraction, error mapping to the wire's failure
type, routing syntax, and the serve loop. HTTP (`#[routes]`), gRPC
(`#[grpc_routes]`), MCP (`#[routes]` MCP members — `#[mcp_routes]` is the deprecated alias — `r2e-mcp`), and scheduled tasks
(`#[scheduled]`, timer-driven) are the existing adapters — use them as
references.

## What a new transport must provide

1. **A parsing + codegen macro** (in `r2e-macros`): a parser for the
   transport's own attributes plus an emitter. Members go through the shared
   member model — `parsing/member.rs`: `parse_member(method, &MemberRules)`
   returns a `MemberDef { name, fn_item, decorators, identity_param, doc,
   deprecated }` that the transport's member type embeds (`RouteMethod`,
   `SseMethod`, `WsMethod`, `McpTool`, `GrpcMethod` all do). `MemberRules`
   carries the per-transport differences (plugin registry, disallowed
   attributes, member markers, legacy identity, duplicate-identity message:
   `MemberRules::http()` / `grpc()` / `mcp(marker)`); impl-level attributes go
   through `validate_impl_attrs` / `parse_decorators`, and
   `reject_self_in_route_signature` lives there too. The struct keeps using
   `#[controller]` unchanged — that macro owns `#[inject]`/`#[config]` fields
   and emits `ContextConstruct`.

2. **A wrapper struct holding the shared core** — build the core ONCE at
   registration via `ContextConstruct::from_context(ctx)`, hold it in an
   `Arc`, clone the `Arc` per call. Never re-resolve beans per request. A
   member that runs on the struct's request scope binds the façade through
   `codegen/identity_path.rs`: `bind_facade(names, core, arrival)` emits
   `let __data = …; let __facade = __r2e_meta_<Name>::bind_request(core,
   __data); let __ctrl = &__facade;`, where the `Arrival` is `ExtractHere`
   (HTTP: `RequestData::extract` from the handler's parts) or `FromDeposit`
   (MCP: the data the endpoint layer deposited in the call parts, or
   `unscoped_request_data` for a service without request scope). The guard
   identity is `identity_path::guard_identity(meta_mod, GuardIdentity::…)`
   (`Param` / `Anonymous` / `Facade`). HTTP `#[anonymous]` routes run on the
   core (`bind_core`); MCP members always run on the façade.

3. **Prebuilt decorator sets** — one storage layout for every
   member-per-method adapter: `codegen::decorators::MemberDecoSets::build(
   controller, adapter, ctrl_guards, ctrl_intercepts, members)` emits one
   shared controller set (kind `"{adapter}CtrlDeco"`, built once per
   service) plus one `DecoSet` per member (kind `"{adapter}Deco"`, field
   `__deco_<fn>`) through `generate_named_deco_items` — gRPC uses
   `"Grpc"`, MCP `"Mcp"` (HTTP routes keep `"Deco"`, scheduled `"Sched"`).
   Every site is validated with `spec_type_of` before anything is emitted
   (one `compile_error!` per bad site, controller sites included on a
   service without members), and the constructors are associated functions
   of the service type (`<Ctrl>::__r2e_mcpdeco_<Ctrl>_<fn>`), so a decorator
   expression may name `Self` — HTTP sets are still free functions.
   The wrapper declares `field_decls()`, registration initializes
   `field_inits()` from the `BeanContext`, and the invoke code reads
   `ctrl_guards(base)` / `member_guards(i, base)` / `intercepts(i, base)`
   (controller sites first) — plain field borrows, no per-call `Arc` clone
   or allocation. Wrap method bodies with `wrap_with_interceptor_refs`.

4. **The `EndpointDeps` carrier** — `codegen/registration.rs`:
   `DepsFold::endpoint(name, site_exprs)` folds every decorator site's spec
   deps onto `ContextConstruct::Deps`, `append(list)` adds transport extras
   (HTTP: `ManagedDeps` of `#[managed]` params via `controller_deps_fold`;
   gRPC: none since #1213 — identity is a witness bound, below), and `carrier_impl(name)` emits
   `impl r2e_core::EndpointDeps for <Name> { type Deps = <fold> }`. This is
   what makes missing beans a compile error. A state-generic transport trait
   assembles its witness with `registration::Witness`: `push_identity(fn,
   declared_ty)` per `#[inject(identity)]` parameter, then `generics()` /
   `tuple()` (`(__R2eMd, __R2eMp<Fn>, …)`) and the marker/identity
   predicates — `Controller<S, W>`, `McpService<S, W>` and (since #1213)
   `GrpcService<S, W>` all use it.
   One type = one `EndpointDeps` impl folding **every** transport's sites
   (#1215): a type that is several endpoint kinds at once gets a single
   carrier whose `Deps` covers all of them, and each transport trait keeps
   its own witness. Today the only multi-transport impl is `#[routes]` with
   MCP members (`attrs/routes_attr.rs` splits the impl by member, parses each
   side with its own parser, then the HTTP codegen emits the one façade impl
   and the one carrier from `controller_site_exprs` + the MCP `site_exprs`;
   the MCP codegen skips its own carrier when `mixed_http_sites` is set, and
   `validate_config` unions both site sets). A new transport that wants to
   share a type with HTTP joins that dispatcher rather than emitting a
   second carrier. gRPC stays a separate macro (`#[grpc_routes]` rejects MCP
   markers).

5. **A registration extension trait with the compile check** — mirror
   `AppBuilderGrpcExt` (`r2e-grpc/src/lib.rs`): inference witnesses on the
   trait, endpoint type on the method:

   ```rust
   pub trait AppBuilderXyzExt<T, DepIdx>: Sized
   where T: Clone + Send + Sync + 'static {
       fn register_xyz_service<S>(self) -> Self
       where
           S: XyzService + EndpointDeps,
           S::Deps: AllSatisfied<T, DepIdx>;
   }
   ```

   Call sites stay `.register_xyz_service::<MyService>()` — never name the
   witnesses. Do NOT ship an unchecked registration path. (MCP adds a
   third witness, `AppBuilderMcpExt<T, W, DepIdx>` with
   `S: McpService<T, W>`: the identity-extractor witness of point 6.)

6. **Wire-specific pieces, kept wire-specific** (do not abstract these —
   deliberate decision):
   - identity extraction — wire-specific only where the wire has no HTTP
     request head. gRPC builds one per call (#1213: `r2e_grpc::__macro_support::into_head`
     — `POST`, `uri` = the method path recorded by the `CaptureMethodPath`
     routing layer, headers = the metadata moved zero-copy, extensions moved +
     `ConnectInfo`) and so reuses the HTTP contract too: `GrpcService<S, W>`
     is state-generic, struct request data and identity parameters extract
     through `RequestData<S>` / `FromRequestPartsVia<S, M>` against the
     state, and the validator bean is checked through the witness. MCP rides HTTP, so it reuses the
     **HTTP contract** (#1175): `McpService<T, W>` is state-generic like
     `Controller<T, W>`, `#[routes]` emits one
     `FromRequestPartsVia<S, M>` bound per member with an
     `#[inject(identity)]` parameter and folds the markers into the witness
     `W`, which `AppBuilderMcpExt<T, W, DepIdx>` infers at
     `.register_mcp_service::<Svc>()` (missing bean = compile error there).
     `McpService::routes(state, ctx)` hands the state to every member's
     extractor, run on an owned copy of the transport request's parts. The
     `mcp.auth` principal is just another extractor (`McpPrincipal`, marker
     `ViaMcpAuth`, `r2e-mcp/src/auth/extract.rs`) — no MCP-only identity
     path. Struct-level request scope (#1210) reuses the `#[controller]`
     items unchanged (`__R2eRequestData_<Name>::extract`, `bind_request`,
     façade, `guard_identity`): the MCP codegen only moves the call point —
     `McpRoutes::request_scope` is a `RequestScope` closure the `McpServer`
     plugin collects into one `McpIdentityLayer` on `/mcp` (runs every
     service's extraction once per HTTP request, deposits the data in the
     request extensions, derives an `McpCaller` {sub, roles} through the
     `CallerProbe` autoref ladder — `RoleBasedIdentity` > `Identity` > none);
     each member binds its façade from the deposit (`r2e-mcp/src/identity.rs`,
     `r2e-macros/src/mcp_codegen/service_impl.rs`). The witness `W` starts
     with the request-data marker, so a plain service is `McpService<S, ((),)>`,
   - the `GuardContext` builder and the `Rejection` → wire-error projection
     (HTTP: the route envelope; gRPC: `GuardContext::from_parts` over the
     call head + `rejection_to_status`; MCP: `member_guard_context` +
     `From<Rejection> for McpError`) — the guard trait itself is shared,
   - error mapping and the serve loop / plugin (registry filled at
     registration, drained ONCE at serve time — see `GrpcServiceRegistry`:
     the `GrpcServer` plugin's `on_serve` hook drains it and spawns tonic on
     the separate port, or an `add_layer` router transform mounts the
     accumulated routes behind `MultiplexService` for the single-port mode —
     `r2e-grpc/src/server.rs`).

7. **Tests** — a trybuild compile-fail for a missing bean at the
   registration call site (model: `grpc_intercept_missing_dep.rs`, which
   typechecks against real generated code — `r2e-compile-tests` compiles
   `proto/ping.proto` via `r2e-grpc-build` in its build.rs and exposes it
   as `r2e_compile_tests::proto`) and a runtime test proving interceptor
   sets are graph-built once (model:
   `examples/example-grpc/tests/grpc_intercept.rs`).

## Invariants (do not break)

- Interceptors (`Interceptor<R>`) are transport-neutral — `Logged`, `Timed`
  and user interceptors must work on every transport unchanged.
- Decorators are built at registration, never per request/call.
- Every registration path compile-checks `EndpointDeps` via `AllSatisfied`
  (or `ModuleDepsSatisfied` for module scope).
- Guards: the rule of three was met by MCP (third wire transport,
  2026-08-27) and resolved the OTHER way — **new adapters with HTTP request
  context reuse `Guard<I>`/`GuardContext` directly** instead of minting a
  per-transport guard trait. MCP builds a `GuardContext` from the transport
  request's `http::request::Parts`; guards return a typed `Rejection`, which
  each wire projects by **kind** (`From<Rejection> for McpError`,
  `r2e_grpc::rejection_to_status` — a free fn, orphan rule) — so `#[guard]`,
  `#[roles]`-style specs, `RateLimitGuard` and every user
  `#[derive(DecoratorBean)]` guard work on MCP members and gRPC methods with
  zero new impls. gRPC joined in #1072 P3 and, since #1213, builds its
  context with `GuardContext::from_parts` over the per-call head (`method`
  `POST`, `uri` the method path, `path_params` empty); the former `GrpcGuard` /
  `GrpcRolesGuard` / `GrpcGuardContext` are gone. A per-transport guard trait
  is no longer an option to reach for — build a `GuardContext` instead.

## Decisions log

- **2026-11-03 (#1215, transport-layering level 3a) — reverses the §4
  "one type = one endpoint kind" constraint.** `#[routes]` accepts MCP
  members (`#[tool]`/`#[resource]`/`#[prompt]`/`#[completion]`) beside HTTP
  routes; a mixed impl is both a `Controller` and an `McpService` with ONE
  `EndpointDeps` impl and explicit dual registration
  (`register_controller` + `register_mcp_service`). Impl-level
  `#[guard]`/`#[roles]`/`#[all_roles]`/`#[intercept]` apply to both
  transports and are built once per transport; `#[pre_guard]`/
  `#[middleware]`/`#[layer]` stay HTTP-only. Implemented by splitting the
  impl and reusing each transport's parser unchanged (no merged parser), so
  HTTP-only and MCP-only expansions are byte-stable. `#[mcp_routes]` became a
  `#[deprecated]` alias through the same dispatcher (removed next minor); an
  empty MCP service is spellable only through it. The macro deprecation lint
  fires at the attribute path: an item-level `#[allow(deprecated)]` on the
  impl does not silence it, a module/crate-level `#![allow(deprecated)]`
  does.

- **2026-11-03 (#1213, transport-layering level 2) — reverses §6 bullet 1
  for gRPC.** "Identity extraction stays wire-specific" was right for a wire
  with no request head; gRPC *has* one (HTTP/2 `:path` + headers), so the
  generated tonic method now turns each call into `http::request::Parts`
  and every extractor — identity, `#[inject(request)]` fields, custom
  `FromRequestPartsVia` types — rides the HTTP contract. `GrpcIdentity` /
  `JwtIdentitySpec` / `guard_context` were removed (compile-time migration:
  a deprecated `GrpcIdentity` stub over an unsatisfiable sealed supertrait
  fails any impl with an E0277 naming `docs/migration/grpc-identity.md`).
  The bullet still holds for a future wire with no HTTP head.

