# Feature 16 — Multipart (File Upload)

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
// Exporte : FromMultipart, Multipart, TypedMultipart, UploadedFile
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
            "Bonjour {}, {} octets recus",
            form.name,
            form.avatar.len()
        ))
    }
}
```

### 4. UploadedFile Structure

```rust
pub struct UploadedFile {
    /// Le nom du champ dans le formulaire.
    pub name: String,
    /// Le nom de fichier original fourni par le client, si present.
    pub file_name: Option<String>,
    /// Le type de contenu (type MIME), si fourni.
    pub content_type: Option<String>,
    /// Les donnees brutes du fichier.
    pub data: Bytes,
}
```

#### Methods

| Method | Return | Description |
|--------|--------|-------------|
| `len()` | `usize` | Size of the data in bytes |
| `is_empty()` | `bool` | Whether the data is empty |

The raw data is accessible via the `data` field (`bytes::Bytes`). File contents are kept in memory — it is recommended to enforce size limits at the web server or reverse proxy level for large uploads.

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

#[derive(Controller)]
#[controller(path = "/uploads", state = Services)]
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

The four error variants:

| Variant | Cause |
|---------|-------|
| `MissingField` | A required field was not present |
| `ParseError` | A text field could not be parsed to the expected type |
| `AxumError` | The underlying Axum multipart extractor failed (e.g., incorrect content-type) |
| `ReadError` | A field's data could not be read (e.g., UTF-8 decoding failure) |

## Raw Multipart Access

For advanced cases where full control over field iteration is needed, use the raw `Multipart` extractor directly:

```rust
use r2e::multipart::Multipart;

#[post("/raw")]
async fn upload_raw(&self, mut multipart: Multipart) -> JsonResult<Value> {
    let mut fields = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        HttpError::BadRequest(format!("erreur multipart: {e}"))
    })? {
        let name = field.name().unwrap_or("unknown").to_string();
        let file_name = field.file_name().map(|s| s.to_string());
        let data = field.bytes().await.map_err(|e| {
            HttpError::Internal(format!("echec de lecture du champ: {e}"))
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
  -F "bio=Developpeur Rust" \
  -F "avatar=@photo.jpg" \
  -F "attachments=@doc1.pdf" \
  -F "attachments=@doc2.pdf"
# → {"name":"Alice","bio":"Developpeur Rust","avatar_size":12345,...}
```
