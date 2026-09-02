---
topic: modules
features: core
tokens: ~2100
requires: di-beans, plugins
---

## Feature Modules — `#[module]`

### TL;DR

- `#[module(...)]` packages a vertical slice as one registration; install it with
  `b.register_module::<M>()` during the **builder phase**, before `build_state()`.
- The keys are `providers`, `controllers`, `grpc_services`, `exports`, `imports`,
  `plugins`, `requires_plugins` — everything the slice does not own must be in
  `imports(...)`.
- An `imports(...)` entry is either a bean type or `module(Other)` (which pulls in
  that module's `exports` without restating them).
- Importing a module does **not** register it — call
  `.register_module::<Other>()` yourself; imported modules are never auto-registered.
- `plugins(Type = expr)` **installs** a plugin (its beans become app-global and
  must NOT be listed in `exports`); `requires_plugins(Type)` only demands one, and
  a missing one is a compile error naming the plugin.
- One owner per plugin: installing the same plugin twice is a startup
  `DuplicatePlugin` error naming both owners.
- `#[module(prefix = "/api/v1")]` prefixes every controller path of the module
  (declared on the module, validated at compile time, no path params or wildcards);
  gRPC and MCP services ignore it.
- `#[module(modules(A, B))]` — or a tuple with `register_modules` — is an
  **aggregate**: a naming device, exclusive with every other `#[module]` key.
- `grpc_services(...)` may inject the module's **private** providers (the app-level
  `.register_grpc_service::<S>()` cannot) and implies the `GrpcServer` plugin; each
  service has exactly one owner.
- A hand-written `FeatureModule` impl must write `type Plugins = (); fn plugins() {}`
  when it brings no plugin and `type Endpoints = ();` when it owns no gRPC service.

Package a vertical slice (services + controllers) as one registration with a
checked closed subgraph:

```rust
#[module(
    providers(UserService),
    controllers(UserController, UserEventConsumer),
    grpc_services(UserGrpcService),       // gRPC services this slice owns
    exports(UserService),                 // visible to the rest of the app
    imports(LocalEventBus, SqlitePool,    // beans: satisfied by the app's provide/load_config
            module(BillingModule)),       // module(...): pulls in BillingModule's exports
    plugins(Scheduler = Scheduler),       // plugins this module BRINGS (installs)
    requires_plugins(Executor),           // plugins this module only NEEDS (installed elsewhere)
)]
pub struct UserModule;

// in App::build, BEFORE build_state() (register_module is a builder-phase call):
# fn __doc(b: AppBuilder, pool: SqlitePool) -> impl Sized {
b.plugin(Executor)                        // requires_plugins(Executor): installed here
 .plugin(GrpcServer::on_port("0.0.0.0:50051"))   // implied by grpc_services(..)
 .provide(LocalEventBus::new()).provide(pool)    // the beans the module imports
 .register_module::<BillingModule>()      // imported modules are NOT auto-registered
 .register_module::<UserModule>()
# }
# fn main() {}
```

An `imports(...)` entry is either a bean type (satisfied by the app's
`provide`/`load_config`) or `module(OtherModule)`, which pulls in that module's
`exports` without restating them. Importing a module only *requires* its exports —
you must still `.register_module::<OtherModule>()` yourself.

`plugins(...)` vs `requires_plugins(...)`:

- `plugins(Type = expr, ...)` — the module **installs** the plugin, exactly as
  `.plugin(expr)` at the `register_module` call site: the plugin's beans become
  app-global (no `exports(..)` entry — and they must NOT be listed there), its
  controllers are mounted, its effects apply at that position in install order.
  The `Type = expr` form is required (the type grows the provision list at
  compile time, the expression is the instance).
- `requires_plugins(Type, ...)` — the module only **needs** the plugin, installed
  by the app, an earlier module, or this module's own `plugins(..)`. A missing one
  is a compile error naming the plugin.
- **One owner per plugin**: installing the same plugin twice (app + module, or two
  modules) is a startup `DuplicatePlugin` error naming both owners. Use
  `requires_plugins` in every module that does not own it.
- Hand-written `FeatureModule` impls (no macro) must write
  `type Plugins = (); fn plugins() {}` when they bring nothing, and
  `type Endpoints = ();` when they own no gRPC service.

**Path-prefixed modules — `#[module(prefix = "/api/v1")]`:**

```rust
#[module(prefix = "/api/v1", controllers(UserController), imports(UserService))]
pub struct V1Module;      // UserController's #[controller(path = "/users")]
                          // now serves /api/v1/users
```

The prefix is declared **on the module**, not at the `register_module` call
site, so it composes through aggregates (which are purely static folds). It
concatenates with each controller's own `path`, is validated at compile time
(must start with `/`, must not end with `/`, no path parameters or wildcards —
those belong on the controller), and a module's `#[fallback]` becomes
prefix-scoped. It also rewrites the collected route metadata, so the OpenAPI
spec and `r2e routes` show the mounted paths. An aggregate takes no prefix (a
compile error): each member carries its own. gRPC and MCP services are not
path-mounted and ignore the prefix.

**Module aggregates — one blueprint line shared by the app and its tests:**

```rust
#[module(modules(UserModule, BillingModule, ReportModule))]
pub struct AppModules;                    // NOT a module: a named list of them

# fn __doc(b: AppBuilder) -> impl Sized {
b.register_modules::<AppModules>()        // == register_module::<Each>() in order
# }
# fn __doc2(b: AppBuilder) -> impl Sized {
b.register_modules::<(UserModule, BillingModule)>()   // tuple sugar, no named type
# }
# fn main() {}
```

`modules(...)` is **exclusive** with every other `#[module]` key (an aggregate
owns no providers, controllers, exports, imports or plugins — mixing them is a
compile error). `register_modules::<A>()` folds `register_module` over the
members left to right, so encapsulation, plugin ownership, `requires_plugins`
checking, brought-plugin installation and ordering are exactly those of the
hand-written chain: an aggregate is a naming device, not a new scope. Members'
`exports` therefore reach the app-global provision list as usual, which is what
lets one member import another member's export. Tuples up to 16 members are
aggregates out of the box.

`grpc_services(...)` is the transport peer of `controllers(...)`: the slice owns
its gRPC services, so they may inject the module's **private** providers (the
app-level `.register_grpc_service::<S>()` cannot — its deps must be in the
application state). The services are dependency-checked module-locally at
`register_module` (deps ⊆ providers ∪ imports, decorator deps included) and
registered by `build_state()` from the retained bean context, in declaration
order after the module's controllers. The key implies the `GrpcServer` plugin:
it is appended to the module's `requires_plugins`, so forgetting
`.plugin(GrpcServer::...)` is a compile error **naming `GrpcServer`** (the
module may also bring it itself with `plugins(GrpcServer = GrpcServer::on_port(..))`).
`requires_plugins` is checked on the plugin's *provisions*, and `GrpcServer`'s
(`GrpcMarker`) cannot be constructed outside r2e-grpc — so there is no way to
hand-`.provide(..)` past that compile error. A hand-written `impl FeatureModule`
that skips `RequiredPlugins` fails at boot instead, with
`BeanError::MissingTransportPlugin` naming the plugin and the module.

Each gRPC service has exactly one owner. Registering the same service twice —
two modules, or a module plus `.register_grpc_service::<S>()` — fails: the
module path returns `BeanError::DuplicateEndpoint` (visible on
`try_build_state()`), the app-level call panics, and listing a service twice in
one `grpc_services(..)` is a macro compile error.

```rust
#[module(providers(GreetingRepo), grpc_services(GreeterService))]  // GreetingRepo is private
pub struct GreetingModule;

# async fn __doc(b: AppBuilder) -> impl Sized {
b.plugin(GrpcServer::on_port("0.0.0.0:50051"))
 .register_module::<GreetingModule>()      // no .register_grpc_service:: needed
 .build_state().await
# }
# fn main() {}
```
