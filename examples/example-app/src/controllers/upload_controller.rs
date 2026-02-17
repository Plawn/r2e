use r2e::prelude::*;
use r2e::multipart::{Multipart, UploadedFile, TypedMultipart};
use serde_json::Value;

#[derive(FromMultipart)]
pub struct ProfileUpload {
    pub name: String,
    pub bio: Option<String>,
    pub avatar: UploadedFile,
    pub attachments: Vec<UploadedFile>,
}

#[derive(Controller)]
#[controller(path = "/uploads", state = crate::state::Services)]
pub struct UploadController;

#[routes]
impl UploadController {
    /// Typed multipart extraction — fields are automatically parsed into the struct.
    #[post("/profile")]
    async fn upload_profile(
        &self,
        TypedMultipart(form): TypedMultipart<ProfileUpload>,
    ) -> JsonResult<Value> {
        let attachment_sizes: Vec<usize> = form.attachments.iter().map(|f| f.len()).collect();
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

    /// Raw multipart — for advanced use cases where full control is needed.
    #[post("/raw")]
    async fn upload_raw(&self, mut multipart: Multipart) -> JsonResult<Value> {
        let mut fields = Vec::new();
        while let Some(field) = multipart.next_field().await.map_err(|e| {
            AppError::BadRequest(format!("multipart error: {e}"))
        })? {
            let name = field.name().unwrap_or("unknown").to_string();
            let file_name = field.file_name().map(|s| s.to_string());
            let content_type = field.content_type().map(|s| s.to_string());
            let data = field.bytes().await.map_err(|e| {
                AppError::Internal(format!("failed to read field: {e}"))
            })?;
            fields.push(serde_json::json!({
                "name": name,
                "file_name": file_name,
                "content_type": content_type,
                "size": data.len(),
            }));
        }
        Ok(Json(serde_json::json!({ "fields": fields })))
    }
}
