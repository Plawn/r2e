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
//! // In your `App::build` (the app state is inferred from the provision list):
//! b.provide(registry)   // for guards (cached check; bean dep of `FgaCheck`)
//!     .provide(backend) // for direct gRPC access (writes, model management)
//!     .build_state()
//!     .await
//!     .register_controllers::<(DocumentController,)>()
//! ```
//!
//! # Architecture
//!
//! The crate is split into three concerns:
//!
//! - **[`OpenFgaRegistry`]** — wraps any [`OpenFgaBackend`](backend::OpenFgaBackend)
//!   and adds decision caching. Only exposes `check()`. Used by the guard.
//! - **[`FgaClient`]** — the typed, schema-first client for handler code:
//!   `grant`/`revoke` (compile-checked subject types, write-through cache
//!   invalidation) and `check`. **This is the idiomatic write path.**
//! - **[`GrpcBackend`]** — the concrete gRPC implementation. Exposes the raw
//!   `openfga-rs` client via [`client()`](GrpcBackend::client) for anything
//!   beyond single tuples (batch writes, conditional tuples, list objects,
//!   model management).
//!
//! ```ignore
//! // Typed write path — compile-checked against the model, cache-safe:
//! let alice = authz::user::id("alice");
//! let doc = authz::document::id("1");
//! fga.grant(&alice, authz::document::viewer, &doc).await?;
//! let allowed = fga.check(&alice, authz::document::viewer, &doc).await?;
//!
//! // Raw gRPC escape hatch (batch/conditional writes, model management):
//! let mut client = backend.client().clone();
//! client.write(tonic::Request::new(/* ... */)).await?;
//! registry.invalidate_object("document:1"); // manual invalidation required
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
//! # Using Guards (schema-first, recommended)
//!
//! Check the `.fga` model into the repo and generate a typed API with
//! [`model!`]; guards then reference relations through the generated module,
//! so a typo'd relation is a **compile error**, not a silent permanent 403:
//!
//! ```ignore
//! use r2e_openfga::FgaCheck;
//!
//! r2e_openfga::model!(pub mod authz = "fga/model.fga");
//!
//! #[controller(path = "/documents")]
//! pub struct DocumentController {
//!     // The FGA guard checks `user:{identity.sub()}`; it pulls the
//!     // `OpenFgaRegistry` bean itself (a compile-checked decorator dep) —
//!     // no `#[inject]` field is needed for the guard to work.
//!     #[inject(identity)] user: AuthenticatedUser,
//! }
//!
//! #[routes]
//! impl DocumentController {
//!     // GET /documents/{doc_id}: `authz::document::viewer` is checked
//!     // against the model, `path::doc_id` against the route path.
//!     #[get("/{doc_id}")]
//!     #[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
//!     async fn get(&self, Path(doc_id): Path<String>) -> Json<Document> { ... }
//!
//!     // Check using query parameter: GET /documents?id=123
//!     #[get("/")]
//!     #[guard(FgaCheck::has(authz::document::viewer).from_query("id"))]
//!     async fn list(&self, Query(q): Query<DocQuery>) -> Json<Vec<Document>> { ... }
//!
//!     // Check using fixed object (e.g., global admin)
//!     #[delete("/all")]
//!     #[guard(FgaCheck::has(authz::system::admin).fixed("system:global"))]
//!     async fn delete_all(&self) -> StatusCode { ... }
//! }
//! ```
//!
//! The generated `authz::MODEL` (schema 1.1 JSON) is the payload to write to
//! the store, so code and store share one source of truth. For dynamic
//! models there is an unchecked escape hatch:
//! `FgaCheck::relation("viewer").on("document")`.
//!
//! An FGA check resolves `user:{identity.sub()}`, so it **requires an
//! authenticated identity**. `FgaCheck` sets
//! [`DecoratorSpec::REQUIRES_IDENTITY`](r2e_core::DecoratorSpec::REQUIRES_IDENTITY)
//! `= true`, so applying it where the identity is statically always `None` — a
//! controller with no `#[inject(identity)]`, or an `#[anonymous]` route without
//! an `Option<..>` identity parameter — is a **compile error** rather than a
//! guaranteed runtime 401. A required or `Option<..>` struct/parameter identity
//! is accepted (the runtime `None` → 401 in [`FgaGuard`] remains the backstop
//! for the optional case).
//!
//! # Object ID Resolution
//!
//! The guard extracts object IDs from the request. You must specify the source:
//!
//! ```ignore
//! // From path parameter: /documents/{doc_id}. The `path::doc_id` descriptor
//! // (generated by `#[routes]`) is compile-checked against the route path;
//! // a raw `"doc_id"` string is also accepted but unchecked.
//! FgaCheck::relation("viewer").on("document").from_path(path::doc_id)
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
pub mod client;
pub mod config;
pub mod error;
pub mod guard;
pub mod registry;
pub mod typed;

// Re-export openfga-rs so users can access raw types.
pub use openfga_rs;

// The `.fga` parser, for standalone use (build scripts, tooling).
pub use r2e_openfga_model as model_parser;

/// Generate a typed authorization API from a checked-in `.fga` model file —
/// see [`typed`] and [`FgaCheck::has`].
pub use r2e_openfga_macros::model;

// Re-exports
pub use backend::{GrpcBackend, MockBackend, OpenFgaBackend};
pub use client::FgaClient;
pub use config::OpenFgaConfig;
pub use error::OpenFgaError;
pub use guard::{
    FgaCheck, FgaCheckBuilder, FgaGuard, FgaObjectBuilder, ObjectResolver, PathParamName,
};
pub use registry::OpenFgaRegistry;
pub use typed::{
    DirectlyAssignable, FgaObject, FgaRel, FgaSubject, FgaType, FgaUserset, FgaWildcard,
    InvalidObjectId,
};

/// Prelude for convenient imports.
pub mod prelude {
    pub use crate::client::FgaClient;
    pub use crate::config::OpenFgaConfig;
    pub use crate::error::OpenFgaError;
    pub use crate::guard::FgaCheck;
    pub use crate::model;
    pub use crate::registry::OpenFgaRegistry;
}
