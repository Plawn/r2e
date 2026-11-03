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
(`#[grpc_routes]`), MCP (`#[mcp_routes]`, `r2e-mcp`), and scheduled tasks
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
   The wrapper declares `field_decls()`, registration initializes
   `field_inits()` from the `BeanContext`, and the invoke code reads
   `ctrl_guards(base)` / `member_guards(i, base)` / `intercepts(i, base)`
   (controller sites first) — plain field borrows, no per-call `Arc` clone
   or allocation. Wrap method bodies with `wrap_with_interceptor_refs`.

4. **The `EndpointDeps` carrier** — `codegen/registration.rs`:
   `DepsFold::endpoint(name, site_exprs)` folds every decorator site's spec
   deps onto `ContextConstruct::Deps`, `append(list)` adds transport extras
   (HTTP: `ManagedDeps` of `#[managed]` params via `controller_deps_fold`;
   gRPC: each `GrpcIdentity::Spec`'s deps), and `carrier_impl(name)` emits
   `impl r2e_core::EndpointDeps for <Name> { type Deps = <fold> }`. This is
   what makes missing beans a compile error. A state-generic transport trait
   assembles its witness with `registration::Witness`: `push_identity(fn,
   declared_ty)` per `#[inject(identity)]` parameter, then `generics()` /
   `tuple()` (`(__R2eMd, __R2eMp<Fn>, …)`) and the marker/identity
   predicates — `Controller<S, W>` and `McpService<S, W>` both use it.
   Constraint: one type = one `EndpointDeps` impl, so a struct cannot be
   two endpoint kinds at once — share logic through a bean instead.

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
     request head. gRPC: `GrpcIdentity` over metadata (a
     `DecoratorSpec`-backed extractor, so the validator bean is a
     compile-time dep of the service). MCP rides HTTP, so it reuses the
     **HTTP contract** (#1175): `McpService<T, W>` is state-generic like
     `Controller<T, W>`, `#[mcp_routes]` emits one
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
     façade, `guard_identity`): `#[mcp_routes]` only moves the call point —
     `McpRoutes::request_scope` is a `RequestScope` closure the `McpServer`
     plugin collects into one `McpIdentityLayer` on `/mcp` (runs every
     service's extraction once per HTTP request, deposits the data in the
     request extensions, derives an `McpCaller` {sub, roles} through the
     `CallerProbe` autoref ladder — `RoleBasedIdentity` > `Identity` > none);
     each member binds its façade from the deposit (`r2e-mcp/src/identity.rs`,
     `r2e-macros/src/mcp_codegen/service_impl.rs`). The witness `W` starts
     with the request-data marker, so a plain service is `McpService<S, ((),)>`,
   - the `GuardContext` builder and the `Rejection` → wire-error projection
     (HTTP: the route envelope; gRPC: `guard_context` +
     `rejection_to_status`; MCP: `member_guard_context` +
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
  zero new impls. gRPC joined in #1072 P3: `guard_context` views the tonic
  metadata as the `HeaderMap` (`MetadataMap: AsRef<HeaderMap>`), `method` is
  `POST`, `uri` is `/`, `path_params` empty; the former `GrpcGuard` /
  `GrpcRolesGuard` / `GrpcGuardContext` are gone. A per-transport guard trait
  is no longer an option to reach for — build a `GuardContext` instead.
