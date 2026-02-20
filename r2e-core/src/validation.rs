use crate::http::response::{IntoResponse, Response};
use crate::http::{Json, StatusCode};
use serde::Serialize;

// ── Error types ────────────────────────────────────────────

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

// ── Autoref specialization for automatic validation ────────

/// Wrapper used by the autoref specialization trick in generated code.
///
/// The generated handler code calls:
/// ```ignore
/// (&__AutoValidator(&value)).__maybe_validate()
/// ```
///
/// Method resolution picks:
/// - `__DoValidate` (direct match) when `T: garde::Validate<Context = ()>` → runs validation
/// - `__SkipValidate` (autoref fallback) when `T` doesn't impl Validate → no-op
pub struct __AutoValidator<'a, T>(pub &'a T);

/// Matched when `T: garde::Validate<Context = ()>` (direct, higher priority).
pub trait __DoValidate {
    fn __maybe_validate(&self) -> Result<(), Response>;
}

impl<T: garde::Validate> __DoValidate for __AutoValidator<'_, T>
where
    T::Context: Default,
{
    fn __maybe_validate(&self) -> Result<(), Response> {
        self.0
            .validate()
            .map_err(|report| convert_garde_report(&report))
    }
}

/// Fallback via autoref (lower priority) — no-op for types without Validate.
pub trait __SkipValidate {
    fn __maybe_validate(&self) -> Result<(), Response>;
}

impl<T> __SkipValidate for &__AutoValidator<'_, T> {
    fn __maybe_validate(&self) -> Result<(), Response> {
        Ok(())
    }
}

fn convert_garde_report(report: &garde::Report) -> Response {
    let mut field_errors = Vec::new();

    for (path, error) in report.iter() {
        let field = {
            let s = path.to_string();
            if s.is_empty() { "value".to_string() } else { s }
        };
        field_errors.push(FieldError {
            field,
            message: error.message().to_string(),
            code: "validation".to_string(),
        });
    }

    let resp = ValidationErrorResponse {
        errors: field_errors,
    };

    let body = serde_json::json!({
        "error": "Validation failed",
        "details": resp.errors,
    });
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

// Re-export garde::Validate for convenience.
pub use garde::Validate;
