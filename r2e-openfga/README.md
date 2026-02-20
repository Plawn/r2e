# r2e-openfga

[OpenFGA](https://openfga.dev/) fine-grained authorization for R2E — Zanzibar-style relationship-based access control.

## Overview

Integrates OpenFGA for fine-grained, relationship-based access control (ReBAC). Check permissions at runtime against an authorization model, supporting patterns like "user X can view document Y" or "members of team Z can edit project W".

The crate provides:
- A pluggable backend trait (`OpenFgaBackend`) with a gRPC implementation and an in-memory mock
- A registry (`OpenFgaRegistry`) with built-in decision caching
- A declarative guard (`FgaGuard`) that integrates with R2E's `#[guard(...)]` system
- A fluent builder for constructing authorization checks

## Usage

Via the facade crate:

```toml
[dependencies]
r2e = { version = "0.1", features = ["openfga"] }
```

## Setup

### Configuration

```rust
use r2e::r2e_openfga::{OpenFgaConfig, OpenFgaRegistry};

let config = OpenFgaConfig::new("http://localhost:8080", "my-store-id")
    .with_model_id("model-id")          // optional, uses latest model if omitted
    .with_api_token("secret-token")     // optional API authentication
    .with_connect_timeout(10)           // connection timeout in seconds (default: 10)
    .with_request_timeout(5)            // request timeout in seconds (default: 5)
    .with_cache(true, 60);              // enable caching with 60s TTL (default)

let registry = OpenFgaRegistry::connect(config).await?;
```

### Register in application state

```rust
AppBuilder::new()
    .provide(registry)
    .build_state::<AppState, _, _>()
    .await
    .register_controller::<DocumentController>()
    .serve("0.0.0.0:3000")
    .await;
```

## Programmatic API

### Authorization checks

```rust
use r2e::r2e_openfga::OpenFgaRegistry;

#[derive(Controller)]
#[controller(path = "/documents", state = AppState)]
pub struct DocumentController {
    #[inject] fga: OpenFgaRegistry,
}

#[routes]
impl DocumentController {
    #[get("/{id}")]
    async fn get(&self, Path(id): Path<String>) -> Result<Json<Doc>, AppError> {
        let allowed = self.fga.check("user:alice", "viewer", &format!("document:{id}")).await?;
        if !allowed {
            return Err(AppError::Forbidden("Access denied".into()));
        }
        // ...
    }
}
```

### Managing relationship tuples

```rust
// Grant access
self.fga.write_tuple("user:alice", "editor", "document:readme").await?;

// Revoke access
self.fga.delete_tuple("user:alice", "editor", "document:readme").await?;

// List all documents a user can view
let docs = self.fga.list_objects("user:alice", "viewer", "document").await?;
```

## Declarative guard

Use the `FgaCheck` builder with `#[guard(...)]` for declarative authorization on handlers:

```rust
use r2e::r2e_openfga::FgaCheck;

#[routes]
impl DocumentController {
    // Check "viewer" relation on "document:{id}" where {id} comes from the path
    #[get("/{id}")]
    #[guard(FgaCheck::relation("viewer").on("document").from_path("id"))]
    async fn get(&self, Path(id): Path<String>) -> Json<Doc> { ... }

    // Check "editor" relation, object ID from query parameter
    #[put("/")]
    #[guard(FgaCheck::relation("editor").on("document").from_query("doc_id"))]
    async fn update(&self, Query(params): Query<Params>) -> Json<Doc> { ... }

    // Check relation with object ID from header
    #[delete("/")]
    #[guard(FgaCheck::relation("owner").on("document").from_header("X-Document-Id"))]
    async fn delete(&self) -> StatusCode { ... }

    // Check against a fixed object
    #[get("/admin")]
    #[guard(FgaCheck::relation("admin").on("system").fixed("system:global"))]
    async fn admin_panel(&self) -> &'static str { "admin" }
}
```

### Object resolution strategies

The guard resolves the object ID from the request context using `ObjectResolver`:

| Method | Source | Example |
|--------|--------|---------|
| `.from_path("id")` | URL path parameter `/{id}` | `/documents/readme` → `document:readme` |
| `.from_query("doc_id")` | Query string `?doc_id=...` | `?doc_id=readme` → `document:readme` |
| `.from_header("X-Doc-Id")` | Request header | `X-Doc-Id: readme` → `document:readme` |
| `.fixed("system:global")` | Static value | Always `system:global` |

### Guard HTTP responses

| Condition | Status | Description |
|-----------|--------|-------------|
| No identity present | 401 | JWT missing or invalid |
| Object cannot be resolved | 400 | Path/query/header param missing |
| Authorization denied | 403 | User lacks the required relation |
| Backend error | 500 | OpenFGA server unreachable or errored |

## Decision caching

Authorization decisions are cached in a thread-safe `DecisionCache` (DashMap-backed with TTL):

```rust
// Enabled by default (60s TTL)
let config = OpenFgaConfig::new(endpoint, store_id);

// Custom TTL
let config = OpenFgaConfig::new(endpoint, store_id)
    .with_cache(true, 120);  // 120 second TTL

// Disable caching
let config = OpenFgaConfig::new(endpoint, store_id)
    .without_cache();
```

Cache behavior:
- `check()` results are cached by `(user, relation, object)` key
- `list_objects()` results are **not** cached
- `write_tuple()` and `delete_tuple()` automatically invalidate all cached decisions for the affected object
- Manual invalidation: `invalidate_object()`, `invalidate_user()`, `clear_cache()`

## Error types

```rust
use r2e::r2e_openfga::OpenFgaError;

match result {
    Err(OpenFgaError::ConnectionFailed(msg)) => { /* server unreachable */ }
    Err(OpenFgaError::ServerError(msg))      => { /* server returned error */ }
    Err(OpenFgaError::Timeout)               => { /* request timed out */ }
    Err(OpenFgaError::InvalidConfig(msg))    => { /* bad configuration */ }
    Err(OpenFgaError::ObjectResolutionFailed(msg)) => { /* guard: param missing */ }
    Err(OpenFgaError::Denied)                => { /* authorization denied */ }
    _ => {}
}
```

## Testing

A `MockBackend` provides an in-memory tuple store for tests:

```rust
use r2e::r2e_openfga::OpenFgaRegistry;

#[tokio::test]
async fn test_authorization() {
    // Create a mock registry (returns both the registry and a handle to the mock)
    let (registry, mock) = OpenFgaRegistry::mock();

    // Seed test data
    mock.add_tuple("user:alice", "viewer", "document:readme");
    mock.add_tuple("user:alice", "editor", "document:draft");

    // Check permissions
    assert!(registry.check("user:alice", "viewer", "document:readme").await.unwrap());
    assert!(!registry.check("user:bob", "viewer", "document:readme").await.unwrap());

    // With caching enabled
    let (registry, mock) = OpenFgaRegistry::mock_with_cache(60);
    mock.add_tuple("user:alice", "viewer", "document:readme");
    assert!(registry.check("user:alice", "viewer", "document:readme").await.unwrap());
}
```

## Backend trait

Implement `OpenFgaBackend` for custom authorization backends:

```rust
use r2e::r2e_openfga::{OpenFgaBackend, OpenFgaError};
use std::pin::Pin;
use std::future::Future;

struct MyBackend;

impl OpenFgaBackend for MyBackend {
    fn check(&self, user: &str, relation: &str, object: &str)
        -> Pin<Box<dyn Future<Output = Result<bool, OpenFgaError>> + Send + '_>> {
        Box::pin(async move { Ok(true) })
    }

    fn list_objects(&self, user: &str, relation: &str, object_type: &str)
        -> Pin<Box<dyn Future<Output = Result<Vec<String>, OpenFgaError>> + Send + '_>> {
        Box::pin(async move { Ok(vec![]) })
    }

    fn write_tuple(&self, user: &str, relation: &str, object: &str)
        -> Pin<Box<dyn Future<Output = Result<(), OpenFgaError>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }

    fn delete_tuple(&self, user: &str, relation: &str, object: &str)
        -> Pin<Box<dyn Future<Output = Result<(), OpenFgaError>> + Send + '_>> {
        Box::pin(async move { Ok(()) })
    }
}

let registry = OpenFgaRegistry::new(MyBackend);
```

## License

Apache-2.0
