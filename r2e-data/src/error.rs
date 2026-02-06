/// Errors that can occur in the data layer.
#[derive(Debug)]
pub enum DataError {
    NotFound(String),
    Database(sqlx::Error),
    Other(String),
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataError::NotFound(msg) => write!(f, "Not found: {msg}"),
            DataError::Database(err) => write!(f, "Database error: {err}"),
            DataError::Other(msg) => write!(f, "Data error: {msg}"),
        }
    }
}

impl std::error::Error for DataError {}

impl From<sqlx::Error> for DataError {
    fn from(err: sqlx::Error) -> Self {
        DataError::Database(err)
    }
}

impl From<DataError> for r2e_core::AppError {
    fn from(err: DataError) -> Self {
        match err {
            DataError::NotFound(msg) => r2e_core::AppError::NotFound(msg),
            DataError::Database(e) => r2e_core::AppError::Internal(e.to_string()),
            DataError::Other(msg) => r2e_core::AppError::Internal(msg),
        }
    }
}
