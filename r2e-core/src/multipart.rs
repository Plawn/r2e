use std::collections::HashMap;

use bytes::Bytes;

use crate::http::extract::{FromRequest, Request};
use crate::http::response::{IntoResponse, Response};
use crate::http::{Json, StatusCode};

/// Re-export the raw Axum multipart extractor for advanced use cases.
pub use axum::extract::Multipart;

// ── Errors ───────────────────────────────────────────────────────────────────

/// Errors that can occur during multipart extraction.
#[derive(Debug)]
pub enum MultipartError {
    /// A required field was not present in the multipart form.
    MissingField(String),
    /// A text field could not be parsed to the expected type.
    ParseError { field: String, message: String },
    /// An error from the underlying Axum multipart extractor.
    AxumError(String),
    /// An error reading a multipart field's data.
    ReadError(String),
}

impl std::fmt::Display for MultipartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(name) => write!(f, "missing required field: {name}"),
            Self::ParseError { field, message } => {
                write!(f, "failed to parse field '{field}': {message}")
            }
            Self::AxumError(msg) => write!(f, "multipart error: {msg}"),
            Self::ReadError(msg) => write!(f, "failed to read field data: {msg}"),
        }
    }
}

impl IntoResponse for MultipartError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.to_string() });
        (StatusCode::BAD_REQUEST, Json(body)).into_response()
    }
}

// ── UploadedFile ─────────────────────────────────────────────────────────────

/// A file received from a multipart form upload.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    /// The field name in the form.
    pub name: String,
    /// The original file name provided by the client, if any.
    pub file_name: Option<String>,
    /// The content type (MIME type) of the file, if provided.
    pub content_type: Option<String>,
    /// The raw file data.
    pub data: Bytes,
}

impl UploadedFile {
    /// Returns the size of the file data in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Returns `true` if the file data is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// ── MultipartFields ──────────────────────────────────────────────────────────

/// Intermediate collection of all fields from a multipart form.
///
/// Used by `FromMultipart` implementations to extract typed fields.
pub struct MultipartFields {
    /// Text fields, keyed by field name. Multiple values per key are supported.
    pub text: HashMap<String, Vec<String>>,
    /// File fields, keyed by field name. Multiple files per key are supported.
    pub files: HashMap<String, Vec<UploadedFile>>,
}

impl MultipartFields {
    /// Consume an Axum `Multipart` extractor and collect all fields.
    pub async fn collect_from(mut multipart: Multipart) -> Result<Self, MultipartError> {
        let mut text: HashMap<String, Vec<String>> = HashMap::new();
        let mut files: HashMap<String, Vec<UploadedFile>> = HashMap::new();

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| MultipartError::AxumError(e.to_string()))?
        {
            let name = field.name().unwrap_or("").to_string();
            let file_name = field.file_name().map(|s| s.to_string());
            let content_type = field.content_type().map(|s| s.to_string());

            let data = field
                .bytes()
                .await
                .map_err(|e| MultipartError::ReadError(e.to_string()))?;

            // Heuristic: if the field has a file_name, treat it as a file upload.
            // Otherwise, treat it as a text field.
            if file_name.is_some() {
                files.entry(name.clone()).or_default().push(UploadedFile {
                    name,
                    file_name,
                    content_type,
                    data,
                });
            } else {
                let text_value = String::from_utf8(data.to_vec())
                    .map_err(|e| MultipartError::ReadError(e.to_string()))?;
                text.entry(name).or_default().push(text_value);
            }
        }

        Ok(Self { text, files })
    }

    /// Take a single required text value for the given field name.
    pub fn take_text(&mut self, name: &str) -> Result<String, MultipartError> {
        self.text
            .get_mut(name)
            .and_then(|v| if v.is_empty() { None } else { Some(v.remove(0)) })
            .ok_or_else(|| MultipartError::MissingField(name.to_string()))
    }

    /// Take an optional text value for the given field name.
    pub fn take_text_opt(&mut self, name: &str) -> Option<String> {
        self.text
            .get_mut(name)
            .and_then(|v| if v.is_empty() { None } else { Some(v.remove(0)) })
    }

    /// Take a single required file for the given field name.
    pub fn take_file(&mut self, name: &str) -> Result<UploadedFile, MultipartError> {
        self.files
            .get_mut(name)
            .and_then(|v| if v.is_empty() { None } else { Some(v.remove(0)) })
            .ok_or_else(|| MultipartError::MissingField(name.to_string()))
    }

    /// Take an optional file for the given field name.
    pub fn take_file_opt(&mut self, name: &str) -> Option<UploadedFile> {
        self.files
            .get_mut(name)
            .and_then(|v| if v.is_empty() { None } else { Some(v.remove(0)) })
    }

    /// Take all files for the given field name.
    pub fn take_files(&mut self, name: &str) -> Vec<UploadedFile> {
        self.files.remove(name).unwrap_or_default()
    }

    /// Take raw bytes for the given field name (from either text or file fields).
    pub fn take_bytes(&mut self, name: &str) -> Result<Bytes, MultipartError> {
        // Try file first, then text
        if let Some(file) = self.take_file_opt(name) {
            return Ok(file.data);
        }
        if let Some(text) = self.take_text_opt(name) {
            return Ok(Bytes::from(text));
        }
        Err(MultipartError::MissingField(name.to_string()))
    }
}

// ── FromMultipart trait ──────────────────────────────────────────────────────

/// Trait for types that can be constructed from multipart form fields.
///
/// Implement this trait manually or use `#[derive(FromMultipart)]` for automatic
/// derivation.
pub trait FromMultipart: Sized {
    fn from_multipart(fields: MultipartFields) -> Result<Self, MultipartError>;
}

// ── TypedMultipart extractor ─────────────────────────────────────────────────

/// An Axum extractor that consumes a `multipart/form-data` request body and
/// deserializes it into a typed struct implementing `FromMultipart`.
///
/// # Example
///
/// ```ignore
/// use r2e::multipart::{TypedMultipart, UploadedFile, FromMultipart};
///
/// #[derive(FromMultipart)]
/// pub struct ProfileUpload {
///     pub name: String,
///     pub avatar: UploadedFile,
/// }
///
/// #[post("/profile")]
/// async fn upload(&self, TypedMultipart(form): TypedMultipart<ProfileUpload>) -> Json<String> {
///     Json(format!("Received file: {} bytes", form.avatar.len()))
/// }
/// ```
pub struct TypedMultipart<T>(pub T);

impl<T, S> FromRequest<S> for TypedMultipart<T>
where
    T: FromMultipart,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let multipart = Multipart::from_request(req, state)
            .await
            .map_err(|rejection| {
                let err = MultipartError::AxumError(rejection.body_text());
                err.into_response()
            })?;

        let fields = MultipartFields::collect_from(multipart)
            .await
            .map_err(|e| e.into_response())?;

        let value = T::from_multipart(fields).map_err(|e| e.into_response())?;

        Ok(TypedMultipart(value))
    }
}
