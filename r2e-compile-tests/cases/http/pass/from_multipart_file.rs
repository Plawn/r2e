use r2e::prelude::*;
use r2e::web::multipart::UploadedFile;

#[derive(FromMultipart)]
pub struct FileUpload {
    pub title: String,
    pub avatar: UploadedFile,
}

fn main() {}
