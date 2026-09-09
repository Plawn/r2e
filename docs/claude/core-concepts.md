# Core Concepts — Controllers, Injection Scopes, Generated Items, Macro Internals

Extracted from `CLAUDE.md` (the hub keeps a compressed summary). This file is the
authoritative detail on the controller model, the four injection scopes, `#[anonymous]`,
the generated items, the lifecycle hooks, and the `r2e-macros` internal layout.

## Inferred Application State

**The application state is inferred** — there is no hand-written state struct. `AppBuilder::new().provide(bean).register::<T>().build_state().await` materializes the compile-time provision list `P` into a type-level HList of resolved beans (the axum state). Beans are read by type: `state.get::<T>()` (via `BeanAccess`, NOT in the prelude — import explicitly) monomorphizes to a fixed-offset field access; `BeanLookup` (`state.bean::<T>() -> Option<T>`) is the witness-free dynamic form used by `ManagedResource`. Guards/interceptors do NOT read the state: they are built once at registration via `DecoratorSpec` (`#[guard]`/`#[intercept]` expressions name a spec type; bean deps are fields, folded into `Controller::Deps` and compile-checked). The resolved graph is also retained as `Arc<BeanContext>` on the typed builder (`bean_context()`). Apps with >~127 registrations need `#![recursion_limit = "512"]` at the crate root.

## Injection Scopes

**Four injection scopes, all resolved at compile time — two app-scoped, two request-scoped:**

- `#[inject]` — App-scoped. Resolved from the bean graph BY TYPE (`ctx.get::<FieldType>()`) at registration. Type must be `Clone + Send + Sync + 'static` and provided/registered on the builder — a missing bean is a compile error at `register_controller`. Lives on the controller core (built once).
- `#[config("key")]` — App-scoped. Resolved from `R2eConfig`. Type must implement `FromConfigValue`. Lives on the controller core.
- `#[inject(identity)]` — Request-scoped. Extracted via `FromRequestParts` (e.g., `AuthenticatedUser`). Type must implement `Identity`. Drives guards/roles. Lives on the per-request façade.
- `#[inject(request)]` — Request-scoped. Any type implementing `FromRequestParts` (e.g. a tenant id, correlation/trace context, a request-scoped handle). Use it for everything request-scoped that is *not* the auth identity. Lives on the per-request façade. (Not modeled in OpenAPI yet.)

`Option<T>` is supported for both `#[inject(identity)]` and `#[inject(request)]`.

**Handler parameter-level identity injection:**

- `#[inject(identity)]` on handler parameters enables mixed controllers (public + protected endpoints), with each endpoint opting into authentication individually.
- **Optional identity:** `#[inject(identity)] user: Option<AuthenticatedUser>` for endpoints working with or without auth.

## `#[anonymous]` — Fail-Closed Auth With Per-Route Opt-Out

A struct-level identity authenticates **every** route by default; mark the public exceptions with `#[anonymous]` (@PermitAll-style). Anonymous routes are emitted on the controller **core** (like consumers/scheduled): identity extraction is skipped entirely (no JWT cost) and reading the identity or any request-scoped field in the body is a compile error. Guards still run there — with `identity: None` unless the route declares its own optional identity param; OpenAPI drops the security requirement unless explicit `#[guard]`s remain. Rejected combinations (compile errors): `#[anonymous]` + `#[roles]`/`#[all_roles]`, + a **required** `#[inject(identity)]` param (an `Option<T>` identity param is allowed — adaptive public route), or on a controller without a **required** struct identity (no identity or `Option<T>` identity = nothing fail-closed to opt out of — const-assert on `STRUCT_IDENTITY_IS_REQUIRED`). Prefer struct identity + `#[anonymous]` for mostly-protected controllers (forgetting the marker fails closed with a 401); use param-level identity for mostly-public ones.

## Controller Declaration — Two Macros

1. `#[controller(path = "...")]` — a transforming attribute on the struct (no `state` key — controllers are state-generic; optional `tag = "..."` sets the OpenAPI tag, which otherwise defaults to the struct name). It strips request-scoped fields from the physical core struct and generates the metadata module, the request-data extractor, the per-request façade, and the `ContextConstruct` impl (always — the core never holds request-scoped fields).
2. `#[routes]` on the impl block — generates Axum handler functions and the state-generic `Controller<S, W>` trait impl (`S: Clone + Send + Sync + 'static + BeanLookup`; `W` carries inferred extraction markers). Route methods run on the generated façade.

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject]  user_service: UserService,
    #[inject(identity)] user: AuthenticatedUser,
    #[config("app.greeting")] greeting: String,
}

#[routes]
#[intercept(Logged::info())]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> {
        Json(self.user_service.list().await)
    }
}
```

## Generated Items (hidden)

- A physical **core** struct (the source struct with request-scoped fields stripped) — holds `#[inject]` + `#[config]` fields plus a hidden `__r2e_decos: DecoSlot` (prebuilt `#[scheduled]`/`#[consumer]`-method interceptor sets, filled at registration via `Controller::fill_decos`), built once into an `Arc` by `register_controller()`. Cores are not literal-constructible — build via `ContextConstruct::from_context`. The controller core reuses the same bean-level transverse machinery (`r2e-macros/src/codegen/transverse.rs`) for `#[scheduled]`/`#[consumer]`/`#[intercept]`/`#[post_construct]` ("the controller core IS a bean").
- `mod __r2e_meta_<Name>` — `type IdentityType`, `const PATH_PREFIX`, `fn guard_identity()`, `fn bind_request()`, `fn validate_config()`.
- `struct __R2eRequestData_<Name><__M>` — state-generic `FromRequestParts` extractor for the request-scoped values (identity + `#[inject(request)]`), extracted through `FromRequestPartsVia<S, M>` (R2E-owned trait with a marker slot where bean-backed extractors park their `HasBean` index witnesses — E0207). Marker-only + infallible when there are none.
- `struct __R2eRequest_<Name>` — the per-request façade: `{ __core: Arc<Core>, <request-scoped fields> }`, with `Deref<Target = Core>`. Route methods run on this; `self.<injected/config>` resolves through `Deref`, `self.<identity/request>` is a direct façade field.
- `impl ContextConstruct for Name` — always generated; `from_context(ctx)` pulls each `#[inject]` field with `ctx.get::<Ty>()` and declares `type Deps` (checked via `AllSatisfied` at registration).
- `impl<S, ...markers> Controller<S, W> for Name` — receives the core built by `register_controller()` (an extension-trait method: `RegisterController`/`RegisterControllers`, in the prelude) and wires routes, consumers, and scheduled tasks to that same instance. Per request: one `Arc` clone of the core + one `FromRequestParts` extraction binding the stack façade. No DI re-resolution per request, no `Extension<Arc<Controller>>`, no task-local identity.

## Lifecycle Hooks

**`#[post_construct]`** — lifecycle hook on `#[bean]` methods **and on `#[routes]` controller impls**. `&self` only, may be async, returns `()` or `Result<(), Box<dyn Error + Send + Sync>>`. Generates a `PostConstruct` trait impl. Timing differs by host: bean hooks run inside `build_state()` (after the graph resolves, before subscribers); controller-core hooks run at startup during `register_controller`/`build_with_consumers`, **before** consumer registrations (later than bean hooks, since cores are built after the graph). An `Err` aborts startup. On controllers, `#[post_construct]` combined with a route/`#[scheduled]`/`#[consumer]` marker, or with params, or with `#[intercept]`, is a compile error.

**`#[on_start]`** — startup observer on `#[bean]` methods **and** `#[routes]`
controller impls. Same signature/rejection rules as `#[post_construct]`, plus an
optional `#[on_start(order = N)]` (`i32`, default 0). Runs at boot **after** the
whole graph and every controller core are built (so a hook may read anything the
app declares) and after consumer registrations, **before** the plugins' serve
hooks, the builder's `.on_start` closures and the TCP bind. All hooks (beans +
controllers) are sorted ascending by `order`, ties in registration order; an
`Err` **aborts boot** like a builder `.on_start` error. A pinned `override_bean`
skips the hook. It runs under `TestApp::boot` and `build_with_consumers` too (a
test boot is a real startup). Generates `impl OnStart` + `register_on_start` on
beans, and the `Controller::on_start(core)` override on controllers.

**`#[pre_destroy]`** — disposal hook (the `@PreDestroy` counterpart of `#[post_construct]`), on `#[bean]` methods **and** `#[routes]` controller impls. Same signature/rejection rules as `#[post_construct]`. Runs at **graceful shutdown** in the async shutdown phase — controller hooks first, then bean hooks, each in reverse registration order. An `Err` is logged and swallowed (never aborts shutdown); a pinned `override_bean` skips the hook. `#[bean]` generates `impl PreDestroy` + `register_pre_destroy`; a controller core (not `Clone`) uses the `Controller::pre_destroy(core)` override. In tests it fires on `TestApp::shutdown().await`, which runs the production shutdown sequence; the router-only `build_with_consumers` has no shutdown, so nothing fires there.

## Macro Crate Internals (r2e-macros)

`src/` is grouped by role: `attrs/` (transforming attribute macros: `bean_attr`, `controller_attr`, `main_attr`, `module_attr`, `producer_attr`, `routes_attr`, `test_suite_attr`), `derives/` (derive macros: `api_error_derive`, `config_derive`, `params_derive`, …), `parsing/` (`controller_parsing`, `routes_parsing`, `grpc_routes_parsing`), `codegen/` (emission: `controller_codegen`, `controller_impl`, `handlers`, `decorators`, `scheduled`, `transverse`, `wrapping`), `model/` (shared parsed-definition types), `util/` (`crate_path`, `type_utils`, `hash_tokens`, `runtime_args`), plus `extract/` and `grpc_codegen/`.

**Controller path:** `lib.rs` → `attrs/controller_attr.rs` → `parsing/controller_parsing.rs` (`ControllerStructDef`) → `codegen/controller_codegen.rs`

**Routes path:** `lib.rs` → `attrs/routes_attr.rs` → `parsing/routes_parsing.rs` (`RoutesImplDef`) → `codegen/` (handlers, controller_impl, …)

**Shared modules:**

- `model/types.rs` — `InjectedField`, `IdentityField`, `RequestField`, `ConfigField`, `RouteMethod`, `ConsumerMethod`, `ScheduledMethod`, etc.
- `model/route.rs` — `HttpMethod` enum and `RoutePath` parser
- `extract/` — attribute extraction (`route`, `consumer`, `scheduled`, `async_exec`, `managed`, `plugins`, `duration`)

**Inter-macro liaison:** `#[controller]` generates `__r2e_meta_<Name>` (with `bind_request`), `__R2eRequestData_<Name>`, and the `__R2eRequest_<Name>` façade. `#[routes]` references these by naming convention and emits route methods on the façade.

**No-op attribute macros:** `#[get]`, `#[any]`, `#[fallback]`, `#[roles]`, `#[anonymous]`, `#[intercept]`, `#[guard]`, `#[consumer]`, `#[scheduled]`, `#[middleware]`, `#[post_construct]`, `#[on_start]`, `#[pre_destroy]`, etc. are no-op `#[proc_macro_attribute]` parsed by `#[routes]` or `#[bean]`. `#[inject]` (incl. `#[inject(identity)]` / `#[inject(request)]`), `#[config]`, and `#[config_section]` are field helper attributes consumed by `#[controller]`.
