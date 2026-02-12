use crate::error::DataError;
use crate::page::{Page, Pageable};
use std::future::Future;

/// Generic async repository trait for CRUD operations.
///
/// Uses RPITIT (return-position `impl Trait` in traits) — no `async-trait` needed.
pub trait Repository<T, ID>: Send + Sync
where
    T: Send + Sync + 'static,
    ID: Send + Sync + 'static,
{
    fn find_by_id(&self, id: &ID) -> impl Future<Output = Result<Option<T>, DataError>> + Send;
    fn find_all(&self) -> impl Future<Output = Result<Vec<T>, DataError>> + Send;
    fn find_all_paged(&self, pageable: &Pageable) -> impl Future<Output = Result<Page<T>, DataError>> + Send;
    fn save(&self, entity: &T) -> impl Future<Output = Result<T, DataError>> + Send;
    fn delete(&self, id: &ID) -> impl Future<Output = Result<bool, DataError>> + Send;
    fn count(&self) -> impl Future<Output = Result<u64, DataError>> + Send;
}
