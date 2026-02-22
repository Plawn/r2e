//! # r2e-data — Backend-agnostic data access abstractions
//!
//! This crate defines the **pure abstraction layer** for R2E's data access:
//! traits, types, and error handling with **zero database driver dependencies**.
//!
//! Concrete backends live in separate crates:
//! - [`r2e-data-sqlx`](https://docs.rs/r2e-data-sqlx) — SQLx backend (SQLite, Postgres, MySQL)
//! - [`r2e-data-diesel`](https://docs.rs/r2e-data-diesel) — Diesel backend (skeleton)
//!
//! # What's in this crate
//!
//! | Type | Description |
//! |------|-------------|
//! | [`Entity`] | Trait mapping a Rust struct to a SQL table (table name, columns, id) |
//! | [`Repository`] | Async CRUD trait (`find_by_id`, `find_all`, `save`, `delete`, `count`) |
//! | [`Page`] | Paginated result wrapper with metadata (content, total, page info) |
//! | [`Pageable`] | Pagination parameters extractable from query strings (page, size, sort) |
//! | [`DataError`] | Type-erased error enum (`NotFound`, `Database`, `Other`) |
//!
//! # Usage
//!
//! Most users should depend on a backend crate (e.g. `r2e-data-sqlx`) which
//! re-exports everything from this crate. Direct dependency on `r2e-data` is
//! only needed when writing backend-agnostic library code.
//!
//! ```ignore
//! use r2e_data::{Entity, Repository, Page, Pageable, DataError};
//!
//! // Define an entity
//! struct User { id: i64, name: String, email: String }
//!
//! impl Entity for User {
//!     type Id = i64;
//!     fn table_name() -> &'static str { "users" }
//!     fn id_column() -> &'static str { "id" }
//!     fn columns() -> &'static [&'static str] { &["id", "name", "email"] }
//!     fn id(&self) -> &i64 { &self.id }
//! }
//! ```
//!
//! # Error bridging
//!
//! [`DataError::Database`] holds a `Box<dyn Error + Send + Sync>`, so backend
//! crates can wrap their driver errors without leaking types. Use
//! [`DataError::database()`] to construct from any error type.
//!
//! `DataError` converts into [`r2e_core::HttpError`] automatically, so you can
//! use `?` in handler return types of `Result<_, HttpError>`.

pub mod entity;
pub mod error;
pub mod page;
pub mod repository;

pub use entity::Entity;
pub use error::DataError;
pub use page::{Page, Pageable};
pub use repository::Repository;

/// Re-exports of the most commonly used data types.
pub mod prelude {
    pub use crate::{DataError, Entity, Page, Pageable, Repository};
}
