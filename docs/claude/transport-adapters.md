# Transport Adapters — Adding a Wire Format to R2E

Status: **REFERENCE** — recorded 2026-07-09, verified against the code
2026-07-23. Read this before wiring a new entry point (a wire protocol, a
message-driven dispatcher, anything that invokes endpoint methods from
outside). Nothing here is pending work: every mechanism described is
implemented.

## The architecture in one paragraph

R2E is ports-and-adapters shaped. The **middle layer is transport-neutral
and already built**: the bean graph (`BeanContext`, `ContextConstruct`),
the decorator machinery (`DecoratorSpec`, `Interceptor<R>`,
`build_decorator`), and the compile-time dep checking (`EndpointDeps` +
`AllSatisfied`). A transport adapter supplies only what is genuinely
wire-specific: request extraction, error mapping to the wire's failure
type, routing syntax, and the serve loop. HTTP (`#[routes]`), gRPC
(`#[grpc_routes]`), and scheduled tasks (`#[scheduled]`, timer-driven) are
the three existing adapters — use them as references.

## What a new transport must provide

1. **A parsing + codegen macro** (in `r2e-macros`), analogous to
   `grpc_codegen/` (the smallest adapter: ~680 lines across
   `grpc_routes_parsing.rs` + `grpc_codegen/{mod,trait_impl,service_impl}.rs`).
   The struct keeps using `#[controller]` unchanged — that macro owns
   `#[inject]`/`#[config]` fields and emits `ContextConstruct`.

2. **A wrapper struct holding the shared core** — build the core ONCE at
   registration via `ContextConstruct::from_context(ctx)`, hold it in an
   `Arc`, clone the `Arc` per call. Never re-resolve beans per request.

3. **Prebuilt interceptor sets** — reuse
   `codegen::decorators::generate_named_deco_items(controller, kind, fn, …)`
   with a distinct `kind` string (HTTP uses `"Deco"`, scheduled `"Sched"`,
   gRPC `"GrpcDeco"`) and wrap method bodies with
   `wrap_with_deco_interceptors`. Sets are built from the `BeanContext` at
   registration, stored behind one `Arc` on the wrapper.

4. **The `EndpointDeps` carrier** — emit
   `impl r2e_core::EndpointDeps for <Name> { type Deps = <fold> }` using
   `codegen::decorators::endpoint_deps_fold(name, site_exprs)` over every
   decorator site. This is what makes missing beans a compile error.
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
   witnesses. Do NOT ship an unchecked registration path.

6. **Wire-specific pieces, kept wire-specific** (do not abstract these —
   deliberate decision):
   - identity extraction (HTTP: `FromRequestParts`; gRPC:
     `GrpcIdentityExtractor` over metadata),
   - a guard trait with the wire's context and reject type (HTTP:
     `Guard<I>` → `HttpError`; gRPC: `GrpcGuard<I>` → `tonic::Status`),
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
- Guards stay per-transport until a third wire transport makes the shared
  abstraction pay for itself (rule of three; `GrpcRolesGuard` duplicating
  `RolesGuard` ~30 lines is the accepted cost).
