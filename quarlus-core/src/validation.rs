use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde::Serialize;
use validator::Validate;

/// A field-level validation error.
#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
    pub code: String,
}

/// Container for validation errors, used as the payload of `AppError::Validation`.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationErrorResponse {
    pub errors: Vec<FieldError>,
}

/// An Axum extractor that deserializes JSON and validates it using `validator::Validate`.
///
/// Drop-in replacement for `Json<T>` — returns a structured 400 response
/// when validation fails.
///
/// # Example
///
/// ```ignore
/// use quarlus_core::validation::Validated;
///
/// async fn create(Validated(body): Validated<CreateUserRequest>) -> Json<User> {
///     // body is guaranteed to pass validation rules
/// }
/// ```
pub struct Validated<T>(pub T);

impl<T, S> FromRequest<S> for Validated<T>
where
    T: DeserializeOwned + Validate + 'static,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let json = axum::Json::<T>::from_request(req, state)
            .await
            .map_err(|rejection: JsonRejection| {
                let err = crate::AppError::BadRequest(rejection.body_text());
                err.into_response()
            })?;

        json.0.validate().map_err(|errors| {
            let field_errors = convert_validation_errors(&errors);
            let resp = ValidationErrorResponse {
                errors: field_errors,
            };
            crate::AppError::Validation(resp).into_response()
        })?;

        Ok(Validated(json.0))
    }
}

fn convert_validation_errors(errors: &validator::ValidationErrors) -> Vec<FieldError> {
    let mut result = Vec::new();
    for (field, field_errors) in errors.field_errors() {
        for error in field_errors {
            result.push(FieldError {
                field: field.to_string(),
                message: error
                    .message
                    .as_ref()
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| format!("Validation failed for field '{field}'")),
                code: error.code.to_string(),
            });
        }
    }
    result
}
