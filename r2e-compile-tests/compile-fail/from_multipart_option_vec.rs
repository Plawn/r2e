use r2e::prelude::*;

#[derive(FromMultipart)]
pub struct BadUpload {
    pub name: String,
    pub attachments: Option<Vec<UploadedFile>>,
}

fn main() {}
