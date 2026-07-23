use r2e::prelude::*;

#[derive(FromMultipart)]
pub struct BasicUpload {
    pub name: String,
    pub description: String,
}

fn main() {}
