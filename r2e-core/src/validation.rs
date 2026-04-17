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

/// Container for validation errors, used as the payload of `HttpError::Validation`.
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

/// Response body shape: `{ "error": "Validation failed", "details": [...] }`.
/// Serialized directly instead of round-tripping through `serde_json::Value`.
#[derive(Serialize)]
struct ValidationErrorBody<'a> {
    error: &'static str,
    details: &'a [FieldError],
}

fn convert_garde_report(report: &garde::Report) -> Response {
    let iter = report.iter();
    let mut field_errors: Vec<FieldError> = Vec::with_capacity(iter.size_hint().0);

    for (path, error) in iter {
        let rendered = path.to_string();
        let field = if rendered.is_empty() {
            String::from("value")
        } else {
            rendered
        };
        field_errors.push(FieldError {
            field,
            message: error.message().to_owned(),
            code: "validation".to_string(),
        });
    }

    let body = ValidationErrorBody {
        error: "Validation failed",
        details: &field_errors,
    };
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

// Re-export garde::Validate for convenience.
pub use garde::Validate;
