use r2e::prelude::*;

#[derive(FromMultipart)]
pub struct OptionalUpload {
    pub name: String,
    pub bio: Option<String>,
    pub nickname: Option<String>,
}

fn main() {}
