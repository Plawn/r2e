# Feature 16 — Multipart (File Upload)

## TL;DR

Typed `multipart/form-data` extraction into a struct. Derive `FromMultipart` on a struct whose field names match the form fields, then take `TypedMultipart<T>` as a handler param — scalar fields are parsed and files collected into `UploadedFile`, with a structured 400 returned before the handler runs if a required field is missing or unparseable. Enable the `multipart` feature (included in `full`); types are in the prelude.


## Goal

Provide typed extraction of multipart forms, transforming `multipart/form-data` requests into Rust structs. Derive `FromMultipart` on a struct and use `TypedMultipart<T>` as a handler parameter — fields are parsed, files are collected, and errors are returned as structured 400 responses.

## Key Concepts

### TypedMultipart

`TypedMultipart<T>` is an Axum extractor. It consumes the request body, collects all multipart fields, and calls `FromMultipart::from_multipart` to construct the struct. If a required field is missing or a value cannot be parsed, a 400 Bad Request is returned before the handler executes.

### UploadedFile

`UploadedFile` represents a single file received from the form. A field is treated as a file when the client sends a `filename` attribute.

### FromMultipart

Derive macro that generates the extraction logic at compile time by inspecting each field's type.

## Usage

### 1. Configuration

Enable the `multipart` feature (included in `full`):

```toml
[dependencies]
r2e = { version = "0.1", features = ["multipart"] }
```

All multipart types are available via the prelude:

```rust
use r2e::prelude::*;
// Exports: FromMultipart, Multipart, MultipartSchema, TypedMultipart, UploadedFile
```

Or import explicitly:

```rust
use r2e::multipart::{TypedMultipart, UploadedFile, FromMultipart};
```

### 2. Defining a Multipart Struct

Derive `FromMultipart` on a struct with named fields. Each field name must match the form field name sent by the client:

```rust
use r2e::prelude::*;

#[derive(FromMultipart)]
pub struct ProfileUpload {
    pub name: String,
    pub avatar: UploadedFile,
}
```

### 3. Using in Handlers

Wrap the struct with `TypedMultipart<T>` in the handler signature:

```rust
#[routes]
impl ProfileController {
    #[post("/profile")]
    async fn upload(
        &self,
        TypedMultipart(form): TypedMultipart<ProfileUpload>,
    ) -> Json<String> {
        Json(format!(
            "Hello {}, {} bytes received",
            form.name,
            form.avatar.len()
        ))
    }
}
```

### 4. UploadedFile Structure

```rust
pub struct UploadedFile {
    /// The field name in the form.
    pub name: String,
    /// The original file name provided by the client, if present.
    pub file_name: Option<String>,
    /// The content type (MIME type), if provided.
    pub content_type: Option<String>,
    /// The raw file data.
    pub data: Bytes,
}
```

#### Methods

| Method | Return | Description |
|--------|--------|-------------|
| `len()` | `usize` | Size of the data in bytes |
| `is_empty()` | `bool` | Whether the data is empty |

The raw data is accessible via the `data` field (`bytes::Bytes`). File contents are kept in memory. Extraction enforces built-in `MultipartLimits` (default: 10 MiB per field, 100 MiB total) — exceeding them yields a 413 response (see error variants below). Use `collect_from_with_limits` for custom caps, and consider additional limits at the web server or reverse proxy level for large uploads.

### 5. Field Type Mapping

The derive macro maps field types to extraction strategies automatically:

| Rust Type | Field Type | Behavior |
|-----------|-----------|----------|
| `String` | Text | Required text field |
| `UploadedFile` | File | Required file field |
| `i32`, `u64`, `bool`, `f64`, ... | Text | Required, parsed via `FromStr` |
| `Bytes` | Text or File | Required, raw bytes |
| `Option<String>` | Text | Optional text, `None` if absent |
| `Option<UploadedFile>` | File | Optional file, `None` if absent |
| `Option<T>` | Text | Optional, parsed via `FromStr` |
| `Option<Bytes>` | Text or File | Optional raw bytes |
| `Vec<UploadedFile>` | File | Zero or more files with the same field name |

**Note:** `Vec<T>` is only supported for `UploadedFile`. For repeated text fields, use the raw `Multipart` extractor.

### 6. Mixed Text and File Fields

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

Text fields without a `filename` attribute are extracted as text and parsed to the target type. Fields with a `filename` attribute are extracted as `UploadedFile`.

### 7. Optional Fields

Wrap any field in `Option<T>` to make it optional. If the field is absent from the form, the value is `None` — no error is returned:

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

### 8. Repeated File Fields

Use `Vec<UploadedFile>` to accept multiple files under the same field name. If the client sends no files for that name, the vector is empty:

```rust
#[derive(FromMultipart)]
pub struct GalleryUpload {
    pub album_name: String,
    pub photos: Vec<UploadedFile>,
}
```

## Complete Example

Controller accepting a profile with optional bio, required avatar, and any number of attachments:

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

## Error Format

When extraction fails, `TypedMultipart` returns an error with a JSON body — 400 Bad Request for missing/parse/read errors, 413 Payload Too Large for size-limit errors:

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

The error variants:

| Variant | HTTP | Cause |
|---------|------|-------|
| `MissingField` | 400 | A required field was not present |
| `ParseError` | 400 | A text field could not be parsed to the expected type |
| `AxumError` | 400 | The underlying Axum multipart extractor failed (e.g., incorrect content-type) |
| `ReadError` | 400 | A field's data could not be read (e.g., UTF-8 decoding failure) |
| `FieldTooLarge` | 413 | A single field exceeded the per-field size limit |
| `PayloadTooLarge` | 413 | The combined payload exceeded the total size limit |

## Raw Multipart Access

For advanced cases where full control over field iteration is needed, use the raw `Multipart` extractor directly:

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

`Multipart` is a re-export of `axum::extract::Multipart`.

## OpenAPI

Multipart endpoints are modeled automatically in the generated spec — no schemars/`JsonSchema` derive needed:

- A `TypedMultipart<T>` parameter produces a `multipart/form-data` request body referencing `#/components/schemas/T`. The schema comes from the `MultipartSchema` impl that `#[derive(FromMultipart)]` generates alongside `FromMultipart`.
- Field mapping: `String` → `string`; `UploadedFile`/`Bytes` → `string` with `format: binary`; `Vec<UploadedFile>` → array of binary strings; integers → `integer`; `f32`/`f64` → `number`; `bool` → `boolean`; any other `FromStr`-parsed type → `string`. `Option<T>` keeps the inner schema and is omitted from `required`; `Vec<UploadedFile>` is also not required (an absent field yields an empty `Vec`).
- A raw `Multipart` parameter is modeled as a free-form `multipart/form-data` object body.
- If you implement `FromMultipart` manually, also implement `r2e::multipart::MultipartSchema` to document the form; without it the endpoint falls back to a schema-less multipart body.

```json
"requestBody": {
  "required": true,
  "content": {
    "multipart/form-data": {
      "schema": { "$ref": "#/components/schemas/ProfileUpload" }
    }
  }
}
```

## Internal Workings

1. `TypedMultipart<T>` implements Axum's `FromRequest` trait
2. It extracts the raw `Multipart` from the request
3. `MultipartFields::collect_from()` iterates all fields and sorts them into text (without filename) and file (with filename)
4. `T::from_multipart()` (generated by the derive macro) retrieves each field by name via `take_text`, `take_file`, `take_files`, etc.
5. Missing required fields and parse failures are returned as `MultipartError`, converted to a 400 response

## Validation Criteria

```bash
curl -X POST http://localhost:3000/uploads/profile \
  -F "name=Alice" \
  -F "bio=Rust developer" \
  -F "avatar=@photo.jpg" \
  -F "attachments=@doc1.pdf" \
  -F "attachments=@doc2.pdf"
# → {"name":"Alice","bio":"Rust developer","avatar_size":12345,...}
```
