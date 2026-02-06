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
//! use r2e_openfga::{OpenFgaConfig, OpenFgaRegistry};
//!
//! // Configure the OpenFGA client
//! let config = OpenFgaConfig::new("http://localhost:8080", "store-id")
//!     .with_cache(true, 60);  // Cache decisions for 60 seconds
//!
//! // Connect to OpenFGA
//! let registry = OpenFgaRegistry::connect(config).await?;
//!
//! // Add to application state
//! AppBuilder::new()
//!     .provide(registry)
//!     .build_state::<Services, _>()
//!     .await
//!     .register_controller::<DocumentController>()
//!     .serve("0.0.0.0:3000").await
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
//!     // Check using query parameter: GET /documents?id=123
//!     #[get("/")]
//!     #[guard(FgaCheck::relation("viewer").on("document").from_query("id"))]
//!     async fn get(&self, Query(q): Query<DocQuery>) -> Json<Document> { ... }
//!
//!     // Check using header: X-Document-Id: 123
//!     #[put("/")]
//!     #[guard(FgaCheck::relation("editor").on("document").from_header("X-Document-Id"))]
//!     async fn update(&self, body: Json<Update>) -> Json<Document> { ... }
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
//! For path-based IDs, extract the path parameter in your handler and do a
//! manual check using the registry:
//!
//! ```ignore
//! #[get("/{id}")]
//! async fn get(&self, Path(id): Path<i64>) -> Result<Json<Document>, AppError> {
//!     let object = format!("document:{}", id);
//!     if !self.fga.check(&format!("user:{}", self.user.sub()), "viewer", &object).await? {
//!         return Err(AppError::Forbidden("Access denied".into()));
//!     }
//!     // ... fetch document
//! }
//! ```
//!
//! # Managing Permissions
//!
//! Use the registry to manage relationship tuples:
//!
//! ```ignore
//! // Grant permission
//! registry.write_tuple("user:alice", "editor", "document:1").await?;
//!
//! // Check permission
//! let can_edit = registry.check("user:alice", "editor", "document:1").await?;
//!
//! // List accessible objects
//! let documents = registry.list_objects("user:alice", "viewer", "document").await?;
//!
//! // Revoke permission
//! registry.delete_tuple("user:alice", "editor", "document:1").await?;
//! ```
//!
//! # Testing
//!
//! Use the mock backend for testing:
//!
//! ```ignore
//! use r2e_openfga::OpenFgaRegistry;
//!
//! let (registry, mock) = OpenFgaRegistry::mock();
//!
//! // Set up test permissions
//! mock.add_tuple("user:alice", "viewer", "document:1");
//!
//! // Run tests...
//! ```

pub mod backend;
pub mod cache;
pub mod config;
pub mod error;
pub mod guard;
pub mod registry;

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
