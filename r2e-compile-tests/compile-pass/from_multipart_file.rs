use r2e::prelude::*;
use r2e::multipart::UploadedFile;

#[derive(FromMultipart)]
pub struct FileUpload {
    pub title: String,
    pub avatar: UploadedFile,
}

fn main() {}
