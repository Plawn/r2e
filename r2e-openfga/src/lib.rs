//! OpenFGA fine-grained authorization for R2E.
//!
//! This crate provides Zanzibar-style relationship-based access control (ReBAC)
//! for R2E applications using [OpenFGA](https://openfga.dev/).
//!
//! # Overview
//!
//! OpenFGA is a high-performance authorization system that implements Google's
//! Zanzibar paper. It allows you to define fine-grained permissions using a
//! relationship-based model:
//!
//! - **Users** have **relations** to **objects**
//! - Example: `user:alice` is a `viewer` of `document:1`
//!
//! # Setup
//!
//! ```ignore
//! use r2e_openfga::{OpenFgaConfig, GrpcBackend, OpenFgaRegistry};
//!
//! let config = OpenFgaConfig::new("http://localhost:8080", "store-id")
//!     .with_cache(true, 60);
//!
//! let backend = GrpcBackend::connect(&config).await?;
//! let registry = OpenFgaRegistry::with_cache(backend.clone(), config.cache_ttl_secs);
//!
//! // Add both to application state
//! AppBuilder::new()
//!     .provide(registry)   // for guards (cached check)
//!     .provide(backend)    // for direct gRPC access
//!     .build_state::<Services, _, _>()
//!     .await
//!     .register_controller::<DocumentController>()
//!     .serve("0.0.0.0:3000").await
//! ```
//!
//! # Architecture
//!
//! The crate is split into two concerns:
//!
//! - **[`OpenFgaRegistry`]** — wraps any [`OpenFgaBackend`](backend::OpenFgaBackend)
//!   and adds decision caching. Only exposes `check()`. Used by the guard.
//! - **[`GrpcBackend`]** — the concrete gRPC implementation. Exposes the raw
//!   `openfga-rs` client via [`client()`](GrpcBackend::client) for full API access
//!   (writes, deletes, list objects, model management, batch operations, etc.).
//!
//! ```ignore
//! // Cached check (via registry)
//! let allowed = registry.check("user:alice", "viewer", "document:1").await?;
//!
//! // Raw gRPC operations (via backend)
//! let mut client = backend.client().clone();
//! client.write(tonic::Request::new(/* ... */)).await?;
//!
//! // Then invalidate the cache
//! registry.invalidate_object("document:1");
//! ```
//!
//! # Custom Backends
//!
//! Implement [`OpenFgaBackend`](backend::OpenFgaBackend) to plug in a custom
//! authorization check (REST proxy, in-process evaluation, etc.):
//!
//! ```ignore
//! use r2e_openfga::{OpenFgaBackend, OpenFgaError};
//!
//! struct MyCustomBackend { /* ... */ }
//!
//! impl OpenFgaBackend for MyCustomBackend {
//!     fn check(&self, user: &str, relation: &str, object: &str)
//!         -> Pin<Box<dyn Future<Output = Result<bool, OpenFgaError>> + Send + '_>>
//!     {
//!         Box::pin(async move { /* your logic */ Ok(true) })
//!     }
//! }
//!
//! let registry = OpenFgaRegistry::with_cache(MyCustomBackend { /* ... */ }, 60);
//! ```
//!
//! # Using Guards
//!
//! The `FgaCheck` builder creates guards for authorization checks:
//!
//! ```ignore
//! use r2e_openfga::FgaCheck;
//!
//! #[derive(Controller)]
//! #[controller(path = "/documents", state = Services)]
//! pub struct DocumentController {
//!     #[inject] fga: OpenFgaRegistry,
//!     #[inject(identity)] user: AuthenticatedUser,
//! }
//!
//! #[routes]
//! impl DocumentController {
//!     // Check using path parameter: GET /documents/{doc_id}
//!     #[get("/{doc_id}")]
//!     #[guard(FgaCheck::relation("viewer").on("document").from_path("doc_id"))]
//!     async fn get(&self, Path(doc_id): Path<String>) -> Json<Document> { ... }
//!
//!     // Check using query parameter: GET /documents?id=123
//!     #[get("/")]
//!     #[guard(FgaCheck::relation("viewer").on("document").from_query("id"))]
//!     async fn list(&self, Query(q): Query<DocQuery>) -> Json<Vec<Document>> { ... }
//!
//!     // Check using fixed object (e.g., global admin)
//!     #[delete("/all")]
//!     #[guard(FgaCheck::relation("admin").on("system").fixed("system:global"))]
//!     async fn delete_all(&self) -> StatusCode { ... }
//! }
//! ```
//!
//! # Object ID Resolution
//!
//! The guard extracts object IDs from the request. You must specify the source:
//!
//! ```ignore
//! // From path parameter: /documents/{doc_id}
//! FgaCheck::relation("viewer").on("document").from_path("doc_id")
//!
//! // From query parameter: ?doc_id=123
//! FgaCheck::relation("viewer").on("document").from_query("doc_id")
//!
//! // From header: X-Document-Id: 123
//! FgaCheck::relation("viewer").on("document").from_header("X-Document-Id")
//!
//! // Fixed object ID (for global resources)
//! FgaCheck::relation("admin").on("system").fixed("system:global")
//! ```
//!
//! **Security:** Dynamic resolvers (path, query, header) reject IDs containing
//! `:` to prevent object type injection. Only the `Fixed` variant accepts
//! pre-formatted `type:id` values.
//!
//! # Testing
//!
//! Use the mock backend for testing:
//!
//! ```ignore
//! use r2e_openfga::{OpenFgaRegistry, MockBackend};
//!
//! let mock = MockBackend::new();
//! mock.add_tuple("user:alice", "viewer", "document:1");
//!
//! let registry = OpenFgaRegistry::new(mock);
//! assert!(registry.check("user:alice", "viewer", "document:1").await.unwrap());
//! ```

pub mod backend;
pub mod cache;
pub mod config;
pub mod error;
pub mod guard;
pub mod registry;

// Re-export openfga-rs so users can access raw types.
pub use openfga_rs;

// Re-exports
pub use backend::{GrpcBackend, MockBackend, OpenFgaBackend};
pub use config::OpenFgaConfig;
pub use error::OpenFgaError;
pub use guard::{FgaCheck, FgaCheckBuilder, FgaGuard, FgaObjectBuilder, ObjectResolver};
pub use registry::OpenFgaRegistry;

/// Prelude for convenient imports.
pub mod prelude {
    pub use crate::config::OpenFgaConfig;
    pub use crate::error::OpenFgaError;
    pub use crate::guard::FgaCheck;
    pub use crate::registry::OpenFgaRegistry;
}
