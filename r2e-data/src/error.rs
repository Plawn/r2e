/// Errors that can occur in the data layer.
#[derive(Debug)]
pub enum DataError {
    NotFound(String),
    Database(Box<dyn std::error::Error + Send + Sync>),
    Other(String),
}

impl DataError {
    /// Construct a `Database` variant from any error type.
    ///
    /// Used by backend crates (e.g. `r2e-data-sqlx`, `r2e-data-diesel`)
    /// to wrap driver-specific errors.
    pub fn database(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        DataError::Database(Box::new(err))
    }
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

impl std::error::Error for DataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DataError::Database(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}

impl From<DataError> for r2e_core::HttpError {
    fn from(err: DataError) -> Self {
        match err {
            DataError::NotFound(msg) => r2e_core::HttpError::NotFound(msg),
            DataError::Database(e) => r2e_core::HttpError::Internal(e.to_string()),
            DataError::Other(msg) => r2e_core::HttpError::Internal(msg),
        }
    }
}
