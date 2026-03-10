# Step 3 — r2e-macros: `#[controller]` Macro

## Goal

Implement the `#[controller]` macro — the centerpiece of the framework. It transforms an annotated `impl` block into complete Axum handlers with state extraction and request-scoped data.

## Files to Create/Modify

```
r2e-macros/src/
  lib.rs              # Add #[controller]
  controller.rs       # Main macro logic
  parsing.rs          # Extraction of inject/identity fields and route methods
  codegen.rs          # Code generation (handlers + impl Controller)
```

## 1. Pipeline Overview

```
Input:                              Output:
#[controller]                       1. Original struct (unchanged)
impl UserResource {                 2. impl UserResource { original methods }
    #[inject]                       3. Free-standing Axum handlers (async functions)
    user_service: UserService,      4. impl Controller<Services> for UserResource
                                    5. fn routes() -> Router
    #[identity]
    user: AuthenticatedUser,

    #[get("/users")]
    async fn list(&self) -> ...
}
```

## 2. Parsing Phase (`parsing.rs`)

### Input

The macro receives a complete `impl` block via `syn::parse`.

### Data to Extract

```rust
pub struct ControllerDef {
    /// Type name (e.g., UserResource)
    pub name: syn::Ident,

    /// #[inject] fields — app-scoped
    pub injected_fields: Vec<InjectedField>,

    /// #[identity] fields — request-scoped
    pub identity_fields: Vec<IdentityField>,

    /// Methods annotated with a route attribute
    pub route_methods: Vec<RouteMethod>,

    /// Non-annotated methods (private helpers, etc.)
    pub other_methods: Vec<syn::ImplItemFn>,
}

pub struct InjectedField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct IdentityField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct RouteMethod {
    pub method: HttpMethod,    // Get, Post, Put, Delete, Patch
    pub path: String,          // "/users/:id"
    pub fn_item: syn::ImplItemFn,
}
```

### Parsing Logic

1. **Identify the type**: extract the `Self` type from the `impl` block
2. **Classify items**:
   - Item with `#[inject]` → `InjectedField`
   - Item with `#[identity]` → `IdentityField`
   - Method with `#[get(...)]` / `#[post(...)]` / etc. → `RouteMethod`
   - Other method → `other_methods`

### Handling Fields in an `impl` Block

Problem: Standard Rust does not allow declaring fields in an `impl` block. Two strategies:

**Strategy A** — Custom syntax in the `impl` block:
The macro parses `field: Type` declarations in the block and removes them from the generated code. The user writes:

```rust
#[controller]
impl UserResource {
    #[inject]
    user_service: UserService,
    // ...
}
```

The macro must parse these items manually since `syn` will not recognize them as valid `ImplItem`s. Use a custom `syn::parse::Parse`.

**Strategy B (alternative)** — Separate struct + attributes:
The user defines the struct normally and `#[controller]` is applied to the `impl`. Injections are declared on the struct:

```rust
#[injectable]
struct UserResource {
    #[inject]
    user_service: UserService,
    #[identity]
    user: AuthenticatedUser,
}

#[controller]
impl UserResource {
    #[get("/users")]
    async fn list(&self) -> Json<Vec<User>> { ... }
}
```

> **Recommendation**: Strategy A to stay true to the target DX from the plan, even though it requires custom parsing.

## 3. Code Generation Phase (`codegen.rs`)

### For each `RouteMethod`, generate an Axum handler

Input:
```rust
#[get("/users")]
async fn list(&self) -> Json<Vec<User>>
```

Output:
```rust
async fn __r2e_handler_list(
    axum::extract::State(state): axum::extract::State<AppState<Services>>,
    user: AuthenticatedUser,    // each #[identity] field
) -> impl axum::response::IntoResponse {
    let controller = UserResource {
        user_service: state.get().user_service.clone(),  // each #[inject] field
        user,                                             // each #[identity] field
    };
    controller.list().await
}
```

### Generation Rules

| Source | In the handler |
|--------|----------------|
| `#[inject] foo: Foo` | `foo: state.get().foo.clone()` |
| `#[identity] bar: Bar` | Extraction parameter: `bar: Bar` |
| Method parameters (besides `&self`) | Additional extraction parameters (e.g., `Path(id): Path<u64>`, `Json(body): Json<T>`) |

### Generate `impl Controller<T>` + `fn routes()`

```rust
impl Controller<Services> for UserResource {
    fn routes() -> axum::Router<AppState<Services>> {
        axum::Router::new()
            .route("/users", axum::routing::get(__r2e_handler_list))
            .route("/users/:id", axum::routing::get(__r2e_handler_get_by_id))
            // ...
    }
}
```

### The `Services` Type (AppState inner)

The concrete type of the AppState inner must be known by the macro. Two options:

**Option 1** — Macro parameter: `#[controller(state = Services)]`
**Option 2** — Naming convention / global type alias
**Option 3** — Inference from `#[inject]` fields (complex)

> **Recommendation**: Option 1 for clarity.

## 4. Handling Method Parameters

Controller methods can have parameters beyond `&self`:

```rust
#[get("/users/:id")]
async fn get_by_id(&self, Path(id): Path<u64>) -> Json<User>
```

These parameters must be **forwarded as-is** as parameters of the generated Axum handler. The macro must:

1. Remove `&self` from the parameter list
2. Keep all other parameters as handler parameters
3. Add identity extraction parameters as well

## 5. Integration in `lib.rs`

```rust
mod controller;
mod parsing;
mod codegen;

#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    controller::expand(args, input)
}

// Auxiliary macros
#[proc_macro_attribute]
pub fn inject(_args: TokenStream, input: TokenStream) -> TokenStream {
    input // no-op, read by #[controller]
}

#[proc_macro_attribute]
pub fn identity(_args: TokenStream, input: TokenStream) -> TokenStream {
    input // no-op, read by #[controller]
}
```

## Validation Criteria

End-to-end compilation test (without a server):

```rust
use r2e_macros::{controller, inject, identity, get};
use r2e_core::{AppState, Controller};

#[derive(Clone)]
struct Services {
    greeting: String,
}

struct HelloController;

#[controller(state = Services)]
impl HelloController {
    #[inject]
    greeting: String,

    #[get("/hello")]
    async fn hello(&self) -> String {
        self.greeting.clone()
    }
}

// Verify that Controller is implemented
let router = HelloController::routes();
```

## Dependencies Between Steps

- Requires: step 0, step 1 (AppState, Controller trait), step 2 (route attributes)
- Blocks: step 5 (example-app)
