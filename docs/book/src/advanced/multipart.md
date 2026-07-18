# Multipart File Uploads

R2E provides typed multipart form extraction, turning `multipart/form-data` requests into regular Rust structs. Derive `FromMultipart` on a struct and use `TypedMultipart<T>` as a handler parameter -- fields are parsed, files are collected, and errors are returned as structured 400 responses.

## Setup

Enable the `multipart` feature (included in `full`):

```toml
[dependencies]
r2e = { version = "0.1", features = ["multipart"] }
```

All multipart types are available through the prelude:

```rust
use r2e::prelude::*;
// Exports: FromMultipart, Multipart, MultipartSchema, TypedMultipart, UploadedFile
```

Or import explicitly:

```rust
use r2e::multipart::{TypedMultipart, UploadedFile, FromMultipart};
```

## Defining a multipart struct

Derive `FromMultipart` on a struct with named fields. The macro inspects each field's type and generates the appropriate extraction logic at compile time.

```rust
use r2e::prelude::*;

#[derive(FromMultipart)]
pub struct ProfileUpload {
    pub name: String,
    pub avatar: UploadedFile,
}
```

Each field name must match the corresponding form field name sent by the client.

## Using in handlers

Wrap your struct with `TypedMultipart<T>` in the handler signature:

```rust
#[routes]
impl ProfileController {
    #[post("/profile")]
    async fn upload(
        &self,
        TypedMultipart(form): TypedMultipart<ProfileUpload>,
    ) -> Json<String> {
        Json(format!(
            "Hello {}, received {} bytes",
            form.name,
            form.avatar.len()
        ))
    }
}
```

`TypedMultipart` is an Axum extractor. It consumes the request body, collects all multipart fields, and calls `FromMultipart::from_multipart` to build your struct. If any required field is missing or a value cannot be parsed, a 400 Bad Request is returned before your handler runs.

## UploadedFile

`UploadedFile` represents a single file received from the form. A field is treated as a file when the client sends a `filename` attribute.

```rust
pub struct UploadedFile {
    /// The field name in the form.
    pub name: String,
    /// The original file name provided by the client, if any.
    pub file_name: Option<String>,
    /// The content type (MIME type), if provided.
    pub content_type: Option<String>,
    /// The raw file data.
    pub data: Bytes,
}
```

### Methods

| Method | Returns | Description |
|--------|---------|-------------|
| `len()` | `usize` | Size of the file data in bytes |
| `is_empty()` | `bool` | Whether the file data is empty |

Access the raw bytes through the `data` field (`bytes::Bytes`). The file content is held in memory. `TypedMultipart` enforces built-in byte caps by default (`MultipartLimits::DEFAULT` — 10 MiB per field, 100 MiB per request); exceeding them returns a `413 Payload Too Large` before your handler runs. For different caps, collect manually via `MultipartFields::collect_from_with_limits` in a custom extractor.

## Field type mapping

The derive macro maps field types to extraction strategies automatically:

| Rust type | Form field kind | Behavior |
|-----------|----------------|----------|
| `String` | Text | Required text field |
| `UploadedFile` | File | Required file field |
| `i32`, `u64`, `bool`, `f64`, ... | Text | Required, parsed via `FromStr` |
| `Bytes` | Text or File | Required, raw bytes from either kind |
| `Option<String>` | Text | Optional text, `None` if absent |
| `Option<UploadedFile>` | File | Optional file, `None` if absent |
| `Option<T>` | Text | Optional, parsed via `FromStr` |
| `Option<Bytes>` | Text or File | Optional raw bytes |
| `Vec<UploadedFile>` | File | Zero or more files with the same field name |

> **Note:** `Vec<T>` is only supported for `UploadedFile`. For repeated text fields, use the raw `Multipart` extractor.

## Mixed text and file fields

A single struct can combine text fields, parsed values, and file uploads:

```rust
#[derive(FromMultipart)]
pub struct DocumentUpload {
    pub title: String,
    pub page_count: u32,
    pub draft: bool,
    pub document: UploadedFile,
}
```

Text fields without a `filename` attribute are extracted as text and parsed into the target type. Fields with a `filename` attribute are extracted as `UploadedFile`.

## Optional fields

Wrap any field in `Option<T>` to make it optional. If the field is absent from the form, the value is `None` -- no error is returned:

```rust
#[derive(FromMultipart)]
pub struct ProfileUpload {
    pub name: String,
    pub bio: Option<String>,
    pub age: Option<u32>,
    pub avatar: UploadedFile,
    pub cover_photo: Option<UploadedFile>,
}
```

## Repeated file fields

Use `Vec<UploadedFile>` to accept multiple files under the same field name. If the client sends no files for that name, the vector is empty:

```rust
#[derive(FromMultipart)]
pub struct GalleryUpload {
    pub album_name: String,
    pub photos: Vec<UploadedFile>,
}
```

## Complete example

Putting it all together -- a controller that accepts a profile with optional bio, a required avatar, and any number of attachments:

```rust
use r2e::prelude::*;
use r2e::multipart::{TypedMultipart, UploadedFile};
use serde_json::Value;

#[derive(FromMultipart)]
pub struct ProfileUpload {
    pub name: String,
    pub bio: Option<String>,
    pub avatar: UploadedFile,
    pub attachments: Vec<UploadedFile>,
}

#[controller(path = "/uploads")]
pub struct UploadController;

#[routes]
impl UploadController {
    #[post("/profile")]
    async fn upload_profile(
        &self,
        TypedMultipart(form): TypedMultipart<ProfileUpload>,
    ) -> JsonResult<Value> {
        let attachment_sizes: Vec<usize> =
            form.attachments.iter().map(|f| f.len()).collect();

        Ok(Json(serde_json::json!({
            "name": form.name,
            "bio": form.bio,
            "avatar_size": form.avatar.len(),
            "avatar_filename": form.avatar.file_name,
            "avatar_content_type": form.avatar.content_type,
            "attachment_count": attachment_sizes.len(),
            "attachment_sizes": attachment_sizes,
        })))
    }
}
```

## Error response format

When extraction fails, `TypedMultipart` returns a 400 Bad Request with a JSON body:

```json
{
    "error": "missing required field: avatar"
}
```

```json
{
    "error": "failed to parse field 'page_count': invalid digit found in string"
}
```

The error variants are:

| Variant | Cause | Status |
|---------|-------|--------|
| `MissingField` | A required field was not present | 400 |
| `ParseError` | A text field could not be parsed to the expected type | 400 |
| `AxumError` | The underlying Axum multipart extractor failed (e.g., content-type mismatch) | 400 |
| `ReadError` | A field's data could not be read (e.g., UTF-8 decoding failure) | 400 |
| `FieldTooLarge` | A single field exceeded the per-field byte limit | 413 |
| `PayloadTooLarge` | The aggregated request payload exceeded the total byte limit | 413 |

## Raw multipart access

For advanced use cases where you need full control over field iteration, use the raw `Multipart` extractor directly:

```rust
use r2e::multipart::Multipart;

#[post("/raw")]
async fn upload_raw(&self, mut multipart: Multipart) -> JsonResult<Value> {
    let mut fields = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        HttpError::BadRequest(format!("multipart error: {e}"))
    })? {
        let name = field.name().unwrap_or("unknown").to_string();
        let file_name = field.file_name().map(|s| s.to_string());
        let data = field.bytes().await.map_err(|e| {
            HttpError::Internal(format!("failed to read field: {e}"))
        })?;
        fields.push(serde_json::json!({
            "name": name,
            "file_name": file_name,
            "size": data.len(),
        }));
    }
    Ok(Json(serde_json::json!({ "fields": fields })))
}
```

`Multipart` is re-exported from axum via `r2e::http::multipart::Multipart`. See the [Axum documentation](https://docs.rs/axum/latest/axum/extract/struct.Multipart.html) for its full API.

## OpenAPI integration

Multipart endpoints appear in the generated OpenAPI spec automatically. A `TypedMultipart<T>` parameter is documented as a `multipart/form-data` request body whose schema is derived from `T`'s fields (`#[derive(FromMultipart)]` also generates a `MultipartSchema` impl — no `JsonSchema` derive required): text fields map to their JSON type, file fields to `type: string, format: binary`, and `Option<...>` fields are not listed as required. A raw `Multipart` parameter is documented as a free-form `multipart/form-data` object. Manual `FromMultipart` impls can opt in by also implementing `r2e::multipart::MultipartSchema`.

## How it works

1. `TypedMultipart<T>` implements Axum's `FromRequest` trait
2. It extracts the raw `Multipart` from the request
3. `MultipartFields::collect_from()` iterates all fields and sorts them into text (no filename) and file (has filename) buckets
4. `T::from_multipart()` (generated by the derive macro) pulls each field by name using `take_text`, `take_file`, `take_files`, etc.
5. Missing required fields and parse failures are returned as `MultipartError`, which converts to a 400 response
