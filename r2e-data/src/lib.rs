pub mod crud;
pub mod entity;
pub mod error;
pub mod migration;
pub mod page;
pub mod query;
pub mod repository;

pub use crud::SqlxRepository;
pub use entity::Entity;
pub use error::DataError;
pub use page::{Page, Pageable};
pub use query::QueryBuilder;
pub use repository::Repository;

pub mod prelude {
    //! Re-exports of the most commonly used data types.
    pub use crate::{Entity, Page, Pageable, QueryBuilder, Repository};
}
