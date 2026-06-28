# Controller Identity Codegen Refactor Roadmap

## Audience and intent

This document is a standalone implementation handoff for Claude Code or a fresh
Codex session. It does not rely on the conversation that produced it. The
implementing agent must first read `CLAUDE.md`, then this entire file, inspect
the working tree, and execute the phases in order. Keep each phase reviewable
and do not redesign the public API while coding unless a decision gate below
explicitly allows it.

For a new Codex session, use a prompt equivalent to:

```text
Read CLAUDE.md and docs/claude/controller-identity-codegen-refactor.md in full.
Preserve the existing working tree. Execute Phase 0 and report the audit, then
implement Phase 1 only. Run the listed validation commands before stopping.
```

The goal is to preserve the convenient controller syntax:

```rust
#[controller(path = "/accounts", state = AppState)]
pub struct AccountController {
    #[inject]
    service: AccountService,

    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl AccountController {
    #[get("/me")]
    async fn me(&self) -> Json<Account> {
        Json(self.service.load(&self.user.sub).await)
    }
}
```

while generating an Axum-facing handler that receives request identity as an
extractor parameter and never reconstructs the application-scoped controller:

```rust,ignore
move |request_data: __R2eRequestData_AccountController, /* Axum args */| {
    let core = captured_core.clone();
    async move {
        let request_controller =
            __r2e_meta_AccountController::bind_request(core, request_data);
        request_controller.me().await
    }
}
```

The target has no runtime reflection, no request `Extension<Arc<Controller>>`,
no task-local identity, and no duplicated full handler body.

## Repository status at handoff

Start by reading `CLAUDE.md` and this file. Do not discard the existing working
tree.

At the time this roadmap was written:

- commit `9da6801` contains the earlier auth/controller performance work;
- the working tree contains an uncommitted V1 Arc-capture implementation in:
  - `r2e-macros/src/codegen/controller_impl.rs`;
  - `r2e-macros/src/codegen/handlers.rs`;
  - `r2e-macros/src/derive_codegen.rs`;
  - `r2e-core/tests/controller_arc_path.rs`;
- the V1 normal path constructs non-identity controllers once and captures an
  `Arc` in Axum closures;
- the V1 still reconstructs controllers with struct-level identity;
- the V1 emits two complete handler variants per endpoint to preserve direct
  `Controller::routes()` compatibility;
- `docs/book/src/advanced/controller-lifecycle-and-dispatch.md` documents that
  intermediate design.

Before editing, run:

```bash
git status --short
git diff --check
cargo test -p r2e-core --test controller_arc_path
cargo test -p r2e-core --test controller_scope
cargo test -p r2e-compile-tests --test compile_tests compile_pass
cargo test -p r2e-compile-tests --test compile_tests compile_fail
cargo check --workspace
```

Do not run `git restore`, `git reset`, or broad formatting over unrelated files.
Do not commit generated `docs/book/book` output.

## Non-negotiable end-state

1. The physical application controller contains only application/config-scoped
   fields and is constructed once during `register_controller()`.
2. A generated request façade owns the concrete identity and an `Arc` to the
   application controller.
3. HTTP/SSE/WebSocket methods execute on the request façade. Their source body
   remains unchanged, including `self.user` and `self.service` field access.
4. Application fields are reached through `Deref<Target = Controller>` from the
   request façade. Identity fields live directly on the façade.
5. The Axum handler receives request data through `FromRequestParts`; identity
   is therefore an explicit generated handler input.
6. The request façade is a stack value. No per-request `Box`, map lookup,
   reflection, or controller dependency reconstruction is allowed.
7. The normal request path performs one `Arc` clone for the controller core.
8. A route has one generated invocation body. Do not retain legacy and Arc
   copies of guards, interceptors, managed resources, method invocation, and
   response conversion.
9. Request identity must never leak across concurrent requests.
10. Consumers and scheduled methods continue to run on the application core and
    cannot access request identity.

## Explicit non-goals

- Do not emulate CDI with `tokio::task_local!`, thread-local storage, global
  context, or a `CurrentIdentity` service locator.
- Do not implement `Deref<Target = AuthenticatedUser>` for a context proxy. A
  safe reference cannot be returned from an unrelated async-local context with
  the lifetime required by `Deref`.
- Do not rewrite every `self.user` expression in method ASTs. That approach is
  fragile around macros, helper methods, borrows, and moves. Move the route
  methods to a generated façade instead.
- Do not preserve direct generated `Controller::routes()` compatibility at the
  cost of a second complete handler. R2E is not in production and `CLAUDE.md`
  explicitly permits breaking changes.
- Do not optimize JWT verification in this refactor. Identity validation and
  controller dispatch are separate concerns.

## Target generated architecture

### 1. Physical controller core

The new struct-level `#[controller(...)]` attribute macro must consume the
source struct and emit a physical struct without request-scoped fields:

```rust,ignore
pub struct AccountController {
    service: AccountService,
}
```

This requires replacing the current combination:

```rust,ignore
#[derive(Controller)]
#[controller(path = "/accounts", state = AppState)]
```

with a real attribute macro:

```rust,ignore
#[controller(path = "/accounts", state = AppState)]
```

The attribute macro sees and can transform the struct. A derive macro cannot
remove fields from its input, so the existing derive-only architecture cannot
implement the target safely.

The migration is intentionally breaking. Do not maintain both systems longer
than needed to migrate the workspace, examples, docs, and compile tests.

### 2. Generated request data extractor

Generate a hidden type for request-only values:

```rust,ignore
#[doc(hidden)]
struct __R2eRequestData_AccountController {
    user: AuthenticatedUser,
}

impl FromRequestParts<AppState> for __R2eRequestData_AccountController {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self {
            user: AuthenticatedUser::from_request_parts(parts, state)
                .await
                .map_err(IntoResponse::into_response)?,
        })
    }
}
```

For controllers without struct identity, still generate a zero-sized request
data type with an infallible extractor. Stable hidden names let `#[routes]`
generate one uniform path without sharing proc-macro state.

### 3. Generated request façade

Generate a hidden façade for every controller:

```rust,ignore
#[doc(hidden)]
struct __R2eRequest_AccountController {
    __core: Arc<AccountController>,
    user: AuthenticatedUser,
}

impl Deref for __R2eRequest_AccountController {
    type Target = AccountController;

    fn deref(&self) -> &Self::Target {
        &self.__core
    }
}
```

Because Rust field access performs autoderef, the unchanged route body can use:

```rust,ignore
self.user       // direct request-façade field
self.service    // application-core field through Deref
```

The façade should own `Arc<AccountController>`, not borrow it. This avoids
lifetime parameters in Axum futures and keeps the façade `Send + Sync` whenever
its fields are `Send + Sync`.

Generate a hidden binder instead of making `#[routes]` know façade fields:

```rust,ignore
fn bind_request(
    core: Arc<AccountController>,
    data: __R2eRequestData_AccountController,
) -> __R2eRequest_AccountController;
```

The binder is generated by the struct macro and moves request fields into the
façade. This is the liaison between the struct and routes macros.

### 4. Method placement

`#[routes]` currently re-emits an `impl AccountController`. Change it to split
methods by execution scope:

- HTTP, SSE, and WebSocket methods: emit on
  `impl __R2eRequest_AccountController`;
- consumers and scheduled methods: emit on `impl AccountController`;
- ordinary helper methods: keep on `impl AccountController` by default;
- interceptor/transaction wrapper methods must be emitted on the same type as
  the route method they wrap.

A route façade can call application-core helper methods through `Deref` as long
as those helpers do not use request identity.

Do not add request-scoped helper support in the first pass. If a helper needs
`self.user`, require it to be inlined into the route or introduce a later,
explicit `#[request_helper]` marker. Never infer a call graph in the proc macro.

Reject or clearly diagnose route methods that expose `Self` in their public
signature until façade semantics for those signatures are deliberately
designed. `Self` inside a moved method would otherwise mean the hidden façade.

### 5. Router construction

Make state-aware construction the only generated controller registration API.
The core runtime trait should converge on:

```rust,ignore
pub trait Controller<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn routes_with_state(state: &S) -> Router<S>;
}
```

It is acceptable to rename this method to `routes` once the old no-argument
method is removed. Prefer one method over retaining aliases.

At router construction:

```rust,ignore
let core = Arc::new(AccountController::from_state(state));

Router::new().route(
    "/me",
    get({
        let core = core.clone(); // once per registered route
        move |data: __R2eRequestData_AccountController, /* args */| {
            let core = core.clone(); // once per request
            async move {
                let controller = bind_request(core, data);
                controller.me(/* args */).await
            }
        }
    }),
)
```

The closure clone performed by Axum may itself clone captured `Arc`s. Inspect
expanded code and runtime counters so the final implementation has one logical
per-request `Arc` increment, not an explicit clone plus a closure clone that
causes a second increment.

Pre-auth guards must be applied while this router is assembled. Do not rebuild
the route through a legacy extractor handler.

## Implementation phases

### Phase 0 — Freeze and understand the V1 baseline

Files to read:

- `r2e-core/src/controller.rs`
- `r2e-core/src/builder.rs`
- `r2e-macros/src/derive_controller.rs`
- `r2e-macros/src/derive_parsing.rs`
- `r2e-macros/src/derive_codegen.rs`
- `r2e-macros/src/routes_attr.rs`
- `r2e-macros/src/routes_parsing.rs`
- `r2e-macros/src/codegen/handlers.rs`
- `r2e-macros/src/codegen/controller_impl.rs`
- `r2e-macros/src/codegen/wrapping.rs`
- `r2e-core/tests/controller_arc_path.rs`
- `r2e-core/tests/controller_scope.rs`

Tasks:

1. Run the baseline commands listed above.
2. Use `cargo expand` on one plain controller, one struct-identity controller,
   and one parameter-identity controller. Store temporary output under `/tmp`,
   not in the repository.
3. Confirm the normal application path enters `routes_with_state()` from
   `AppBuilder::register_controller()`.
4. Confirm exactly where guards, interceptors, managed resources, SSE, WS, and
   pre-auth guards are emitted before moving code.
5. Commit the current V1 separately if it is not already committed. Do not mix
   baseline Arc capture with the façade refactor.

Exit criterion: baseline tests pass and the current expanded shapes are
understood.

### Phase 1 — Remove the duplicate full handler body

Do this before changing public controller syntax.

Refactor `r2e-macros/src/codegen/handlers.rs` into three conceptual layers:

1. `HandlerShape`: parsed/extracted arguments and forwarding order;
2. one invocation-body emitter: guards, interceptors, managed resources,
   method call, response conversion;
3. thin Axum adapters: extractor binding and captured-Arc binding.

Generate one internal invocation function per route:

```rust,ignore
async fn __r2e_invoke_AccountController_me(
    controller: &AccountController,
    /* already extracted/owned arguments */
) -> ResponseOrOriginalType {
    // Existing generated body, emitted once.
}
```

Both temporary V1 adapters must own their controller source for the entire
`.await` and pass only `&Controller` to the invocation function.

For the legacy extractor, generate an `as_controller(&self) -> &Controller`
method so the adapter does not depend on whether the extractor stores
`Controller` or `Arc<Controller>`.

Cover HTTP, SSE, and WS before ending the phase. Do not leave identity
controllers emitting an unused full Arc variant.

Tests:

- all existing runtime and trybuild tests;
- add a compile-pass controller combining identity parameter, guard,
  interceptor, and managed parameter;
- inspect `cargo expand` and verify the large invocation body appears once per
  endpoint.

Exit criterion: behavior is unchanged, but duplicate bodies are gone.

### Phase 2 — Simplify the runtime controller trait

Because breaking changes are allowed, remove the compatibility mechanism that
forces two registration paths.

Tasks:

1. Remove the no-state generated `Controller::routes()` path or change the
   trait to require only a state-aware router constructor.
2. Update manual `Controller` implementations in the workspace.
3. Fold `apply_pre_auth_guards` into state-aware generated route construction,
   or retain it only as a helper called by that single path.
4. Remove legacy `Extension<Arc<Controller>>` lookup and fallback construction.
5. Delete dead `CtrlBinding::Extractor` code after all call sites migrate.
6. Update docs that advertise direct `C::routes()`.

Do not remove raw-Axum escape hatches generally; only remove the generated
compatibility API that prevents a single dispatch path.

Exit criterion: generated controllers have one state-aware registration path
and no request-extension controller lookup.

### Phase 3 — Replace the derive with a transforming controller attribute

Implement a real `#[controller(...)]` proc-macro attribute in
`r2e-macros/src/lib.rs`.

Recommended module split:

```text
r2e-macros/src/controller_attr.rs
r2e-macros/src/controller_parsing.rs
r2e-macros/src/controller_codegen.rs
```

Reuse parsing and field-resolution logic from `derive_parsing.rs` and
`derive_codegen.rs`; do not copy it permanently. Move shared structures into a
neutral module and delete the obsolete derive path after workspace migration.

The attribute macro must:

1. parse controller path/state and all field scopes;
2. remove identity fields from the emitted physical controller;
3. keep injected/config fields on the physical controller;
4. generate `StatefulConstruct<State>` for the physical controller;
5. generate the stable request-data type;
6. generate the stable request-façade type;
7. generate `Deref<Target = Controller>`;
8. generate the request binder;
9. generate metadata/config validation previously emitted by the derive;
10. preserve visibility, generics constraints currently supported, attributes,
    docs, and useful source spans.

Migrate the entire workspace in one mechanical pass:

```rust,ignore
// Before
#[derive(Controller)]
#[controller(path = "/x", state = AppState)]
struct X { ... }

// After
#[controller(path = "/x", state = AppState)]
struct X { ... }
```

Do not keep ambiguous helper-attribute resolution between the old derive and
the new attribute macro.

Exit criterion: identity-free controllers compile through the new attribute
macro with behavior unchanged.

### Phase 4 — Move request methods to the generated façade

Update routes parsing/codegen so route-scoped methods are emitted on the hidden
request façade.

Tasks:

1. Split the original impl into request and application impl blocks.
2. Put HTTP/SSE/WS methods and their generated wrappers on the façade.
3. Keep consumers, scheduled methods, and ordinary core helpers on the core.
4. Change the single Axum adapter to extract generated request data, bind the
   façade, and invoke its method.
5. Change guard identity access to read from the façade.
6. Ensure param-level identity remains a separate route parameter and does not
   make the core request-scoped.
7. Support required and optional struct identity.
8. Preserve middleware/layer order exactly.
9. Preserve pre-auth semantics: pre-auth runs before identity extraction when
   that is the current documented ordering.
10. Add direct diagnostics for illegal request identity use from consumers or
    scheduled methods.

Do not AST-rewrite route bodies. The unchanged body must type-check because the
new receiver is the façade.

Exit criterion: the original `self.user` example authenticates independently
for concurrent requests while the core constructor count remains one.

### Phase 5 — Consolidate route registration codegen

Once only one dispatch path remains, remove parallel registration generators in
`controller_impl.rs`.

Create one registration representation per endpoint containing:

- method/path;
- handler closure expression;
- middleware layers;
- direct layers;
- pre-auth middleware;
- route metadata.

Render HTTP/SSE/WS differences from that representation instead of maintaining
separate legacy/Arc registration trees.

Delete obsolete identifiers and comments such as `_arc`, `legacy`,
`build_arc_router_or`, and `ARC_HANDLER_SUFFIX` when they no longer describe the
architecture.

Exit criterion: there is one obvious code path from a parsed route to its Axum
registration.

### Phase 6 — Documentation, migration, and performance proof

Update at least:

- `CLAUDE.md` generated-items and macro-internals sections;
- `docs/book/src/core-concepts/controllers.md`;
- `docs/book/src/core-concepts/dependency-injection.md`;
- `docs/book/src/advanced/performance.md`;
- `docs/book/src/advanced/controller-lifecycle-and-dispatch.md`;
- examples and README snippets using the old derive syntax.

Document the physical distinction clearly:

- application controller core: constructed once;
- generated request façade: constructed on the stack per request;
- identity: extracted once per request;
- param-level identity: recommended for mixed public/protected controllers;
- struct-level identity: authenticates every endpoint but no longer rebuilds
  application dependencies.

Add a benchmark or reproducible load-test fixture comparing:

1. plain Axum handler;
2. R2E controller without identity;
3. R2E param-level identity with a stub extractor that excludes crypto cost;
4. R2E struct-level identity façade with the same stub extractor.

Measure dispatch separately from JWT verification. Report results; do not add
unverified nanosecond claims to documentation.

Exit criterion: docs match expanded code and measured cost.

## Test matrix

### Runtime behavior

Extend `r2e-core/tests/controller_arc_path.rs` or replace it with a clearly
named façade-focused test file. Cover:

| Scenario | Required assertion |
|----------|--------------------|
| Plain controller | Core constructor runs once across multiple requests |
| Struct identity | Core constructor runs once across multiple requests |
| Concurrent identities | Each response sees its own subject; no cross-request leak |
| Parameter identity | Core remains application-scoped |
| Optional identity | Authenticated and anonymous requests both work |
| Guard | Guard receives the same identity as the method |
| Pre-auth guard | Ordering and rejection occur before route invocation |
| Interceptor | Before/after hooks run once and in the existing order |
| Managed resource | Commit/rollback behavior is unchanged |
| SSE | Façade stays alive for stream setup and identity is correct |
| WebSocket | Upgrade path receives correct identity without rebuilding core |
| Config/injected fields | Core values are available through façade `Deref` |
| Request extensions | No `Arc<Controller>` request extension is installed |

Use atomics/barriers for construction and concurrency assertions. Avoid tests
that depend only on timing.

### Compile-pass

Add/update cases under `r2e-compile-tests/compile-pass/` for:

- new `#[controller]` attribute syntax;
- identity-free controller;
- required struct identity using `self.user`;
- optional struct identity;
- mixed controller with parameter identity;
- core helper called from a request-façade method;
- guards/interceptors/managed parameters;
- SSE and WS identity paths;
- consumer/scheduled methods on an identity-declaring controller when they use
  core fields only.

### Compile-fail

Add/update cases under `r2e-compile-tests/compile-fail/` for:

- duplicate struct identity fields;
- request identity accessed by a consumer/scheduled method;
- unsupported `Self` exposure from a façade route method;
- request-only helper usage if `#[request_helper]` is not implemented;
- old derive/helper syntax, with an actionable migration diagnostic if
  technically possible;
- invalid controller attribute and missing state.

Keep `.stderr` diagnostics intentional. Do not accept noisy compiler cascades
when the proc macro can produce one targeted error.

## Validation commands after every phase

Run the narrowest relevant test first, then the full gates:

```bash
cargo test -p r2e-core --test controller_arc_path
cargo test -p r2e-core --test controller_scope
cargo test -p r2e-compile-tests --test compile_tests compile_pass
cargo test -p r2e-compile-tests --test compile_tests compile_fail
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
git diff --check
```

Also check the feature called out by repository guidance:

```bash
cargo check -p r2e-core --features dev-reload
```

If mdBook sources change:

```bash
mdbook build -d /tmp/r2e-book docs/book
```

Do not regenerate the tracked `docs/book/book` directory with a different
mdBook version.

## Suggested commit sequence

Keep commits independently reviewable:

1. `perf: capture application controllers in generated routes`
   - current V1 only, if still uncommitted;
2. `refactor(macros): share generated handler invocation bodies`
   - no public behavior change;
3. `refactor(core): require state-aware controller routes`
   - explicit breaking change;
4. `refactor(macros): replace controller derive with attribute macro`
   - migrate identity-free controllers first;
5. `perf(macros): generate request facade for controller identity`
   - struct identity no longer reconstructs the core;
6. `refactor(macros): consolidate route registration generation`
   - remove legacy/Arc duplication;
7. `test: cover request facade scope and identity isolation`;
8. `docs: document controller core and request facade lifecycle`.

Do not combine the structural macro migration, all workspace syntax changes,
and behavior changes into one unreviewable commit.

## Decision gates and stop conditions

Stop and report before proceeding if any of these occur:

1. Moving methods to the façade changes a supported public signature involving
   `Self`, associated types, or trait implementations that is used in the
   workspace. Inventory usages and propose a precise rule first.
2. Axum requires the façade itself to implement an extractor in a way that adds
   a second `Arc` clone or request extension lookup. Rework the closure/request
   data boundary instead of accepting hidden overhead.
3. Pre-auth middleware ordering changes relative to identity extraction.
4. SSE/WS futures require request data to outlive owned façade values. Preserve
   ownership; do not introduce unsafe references.
5. A proc-macro implementation needs global mutable state to communicate
   between `#[controller]` and `#[routes]`. Do not use global proc-macro state;
   use stable generated type/function names.
6. The only way to retain an old API is to restore duplicated complete handler
   generation. Prefer the documented breaking change.

## Final definition of done

The refactor is complete only when all statements below are true:

- the source-level controller can declare `#[inject(identity)] user` and use
  `self.user` inside route methods;
- the emitted physical core has no identity field;
- the core is constructed exactly once per registration;
- each request receives a newly extracted identity in an owned stack façade;
- application fields remain accessible through façade autoderef;
- there is one full generated invocation body per endpoint;
- no request controller extension lookup exists;
- no async-local/global identity lookup exists;
- guards, interceptors, managed resources, pre-auth, SSE, and WS pass their
  regression tests;
- concurrent identity-isolation tests pass;
- compile-pass/fail suites and the entire workspace pass;
- expanded code and documentation describe the same architecture;
- dispatch benchmarks separate framework overhead from JWT cryptography.
