use r2e_data::DataError;

/// Extension trait for converting `sqlx::Error` into `DataError`.
///
/// Due to Rust's orphan rules, we can't implement `From<sqlx::Error> for DataError`
/// in this crate. Instead, use `.into_data_error()` or the `?` operator with `SqlxResult`.
pub trait SqlxErrorExt {
    fn into_data_error(self) -> DataError;
}

impl SqlxErrorExt for sqlx::Error {
    fn into_data_error(self) -> DataError {
        match &self {
            sqlx::Error::RowNotFound => DataError::NotFound("Row not found".into()),
            _ => DataError::database(self),
        }
    }
}

/// Convenience alias for data-layer results using `DataError`.
pub type SqlxResult<T> = Result<T, DataError>;
