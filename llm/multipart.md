---
topic: multipart
features: multipart
tokens: ~300
requires: core-concepts
---

## Multipart / File Upload

### TL;DR

- Requires feature `multipart`.
- Derive `FromMultipart` on the form struct and take `TypedMultipart(form): TypedMultipart<T>` as the handler parameter.
- Field types map directly: `String` = required text, `Option<String>` = optional text, `UploadedFile` = required file, `Vec<UploadedFile>` = many files.
- `UploadedFile` exposes `name`, `file_name`, `content_type`, `data: Bytes`, `len()`; the raw `Multipart` extractor is available when the typed form does not fit.

Requires feature: `multipart`

```rust
use serde_json::{json, Value};

#[derive(FromMultipart)]
pub struct ProfileUpload {
    pub name: String,                    // required text field
    pub bio: Option<String>,             // optional text field
    pub avatar: UploadedFile,            // required file
    pub attachments: Vec<UploadedFile>,  // multiple files
}

#[controller(path = "/uploads")]
pub struct UploadController;

#[routes]
impl UploadController {
    #[post("/profile")]
    async fn upload(&self, TypedMultipart(form): TypedMultipart<ProfileUpload>) -> JsonResult<Value> {
        Ok(Json(json!({ "name": form.name, "files": form.attachments.len() })))
    }
}
# fn main() {}
```

`UploadedFile`: `name`, `file_name`, `content_type`, `data: Bytes`, `len()`.
Raw `Multipart` extractor also available.
