use r2e::prelude::*;
use r2e::web::multipart::UploadedFile;

#[derive(FromMultipart)]
pub struct MultiFileUpload {
    pub name: String,
    pub bio: Option<String>,
    pub avatar: UploadedFile,
    pub attachments: Vec<UploadedFile>,
}

fn main() {}
