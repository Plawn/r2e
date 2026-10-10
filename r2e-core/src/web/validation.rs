use crate::error::Rejection;
use serde::{Deserialize, Serialize};

// ── Error types ────────────────────────────────────────────

/// A field-level validation error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
    pub code: String,
}

/// Container for validation errors, the `response` of `HttpError::Validation`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationErrorResponse {
    pub errors: Vec<FieldError>,
}

impl ValidationErrorResponse {
    /// Flatten a `garde` report into field errors (`code` is always
    /// `"validation"`; an empty path renders as `value`).
    #[must_use]
    pub fn from_report(report: &garde::Report) -> Self {
        let iter = report.iter();
        let mut errors: Vec<FieldError> = Vec::with_capacity(iter.size_hint().0);
        for (path, error) in iter {
            let rendered = path.to_string();
            let field = if rendered.is_empty() {
                String::from("value")
            } else {
                rendered
            };
            errors.push(FieldError {
                field,
                message: error.message().to_owned(),
                code: "validation".to_string(),
            });
        }
        Self { errors }
    }
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
///
/// The error is boxed so the `Ok` path is not penalized by a
/// `Rejection`-sized `Result` (clippy `result_large_err`); the generated
/// handler dereferences the box and projects the rejection through the
/// route's error envelope.
pub trait __DoValidate {
    fn __maybe_validate(&self) -> Result<(), Box<Rejection>>;
}

impl<T: garde::Validate> __DoValidate for __AutoValidator<'_, T>
where
    T::Context: Default,
{
    fn __maybe_validate(&self) -> Result<(), Box<Rejection>> {
        self.0
            .validate()
            .map_err(|report| Box::new(Rejection::from(&report)))
    }
}

/// Fallback via autoref (lower priority) — no-op for types without Validate.
pub trait __SkipValidate {
    fn __maybe_validate(&self) -> Result<(), Box<Rejection>>;
}

impl<T> __SkipValidate for &__AutoValidator<'_, T> {
    fn __maybe_validate(&self) -> Result<(), Box<Rejection>> {
        Ok(())
    }
}

// Re-export garde::Validate for convenience.
pub use garde::Validate;
