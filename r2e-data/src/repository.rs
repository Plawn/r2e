use crate::error::DataError;
use crate::page::{Page, Pageable};

/// Generic async repository trait for CRUD operations.
#[async_trait::async_trait]
pub trait Repository<T, ID>: Send + Sync
where
    T: Send + Sync + 'static,
    ID: Send + Sync + 'static,
{
    async fn find_by_id(&self, id: &ID) -> Result<Option<T>, DataError>;
    async fn find_all(&self) -> Result<Vec<T>, DataError>;
    async fn find_all_paged(&self, pageable: &Pageable) -> Result<Page<T>, DataError>;
    async fn save(&self, entity: &T) -> Result<T, DataError>;
    async fn delete(&self, id: &ID) -> Result<bool, DataError>;
    async fn count(&self) -> Result<u64, DataError>;
}
