# Step 2 — r2e-macros: Route Attributes

## Goal

Implement the attribute macros `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]`. These macros do not transform the code themselves — they **mark** methods so that the `#[controller]` macro (step 3) can identify them and generate the Axum handlers.

## Files to Create/Modify

```
r2e-macros/src/
  lib.rs             # Proc-macro entry point, declares the attributes
  route.rs           # Route attribute parsing
```

## 1. Route Attributes (`route.rs`)

Each route attribute captures the **HTTP path** and the **HTTP method**.

### Parsing

```rust
pub struct RouteAttribute {
    pub method: HttpMethod,
    pub path: String,
}

pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}
```

The path is extracted from the attribute arguments:

```rust
// #[get("/users/:id")]  →  RouteAttribute { method: Get, path: "/users/:id" }
```

### Macro Implementation

Each macro (`#[get]`, `#[post]`, etc.) is an **attribute proc-macro** that:

1. Parses the path from the `TokenStream` arguments
2. Annotates the method with a recognizable custom attribute (e.g., `#[r2e_route(method = "GET", path = "/users")]`)
3. Returns the method unchanged (the actual transformation is done by `#[controller]`)

```rust
#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    route_attribute(HttpMethod::Get, args, input)
}

#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    route_attribute(HttpMethod::Post, args, input)
}

// ... same for put, delete, patch
```

### Annotation Strategy

**Option A** — Inert attribute: the `#[get]` etc. macros re-emit the method with a `#[doc(hidden)]` attribute containing the encoded route metadata. The `#[controller]` macro then reads this attribute.

**Option B (recommended)** — No transformation: the `#[get]` etc. macros are pure **no-ops**. The `#[controller]` macro directly parses the `get`, `post`, etc. attributes on the methods of the `impl` block.

Option B is simpler because `#[controller]` receives the complete `impl` block with all attributes intact.

## 2. Entry Point (`lib.rs`)

```rust
extern crate proc_macro;
use proc_macro::TokenStream;

mod route;

#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    // No-op: returns input as-is
    // The path is read by #[controller]
    input
}

#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn put(args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn delete(args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn patch(args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
```

> **Note**: Even if the macros are no-ops, they must still exist as proc-macro attributes so the compiler does not reject `#[get("/path")]` as an unknown attribute.

## Validation Criteria

```rust
use r2e_macros::get;

struct Foo;

impl Foo {
    #[get("/hello")]
    async fn hello(&self) -> String {
        "hello".to_string()
    }
}
```

Compiles without errors (the attribute is accepted and does not modify the method).

## Dependencies Between Steps

- Requires: step 0
- Blocks: step 3 (controller macro)
