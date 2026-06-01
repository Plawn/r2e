use r2e_data::DataError;

/// Extension trait for converting `diesel::result::Error` into `DataError`.
pub trait DieselErrorExt {
    fn into_data_error(self) -> DataError;
}

impl DieselErrorExt for diesel::result::Error {
    fn into_data_error(self) -> DataError {
        match &self {
            diesel::result::Error::NotFound => DataError::NotFound("Row not found".into()),
            _ => DataError::database(self),
        }
    }
}
