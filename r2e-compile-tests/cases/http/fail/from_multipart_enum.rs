use r2e::prelude::*;

#[derive(FromMultipart)]
pub enum BadUpload {
    Text(String),
    File(Vec<u8>),
}

fn main() {}
