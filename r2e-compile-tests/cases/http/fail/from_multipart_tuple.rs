use r2e::prelude::*;

#[derive(FromMultipart)]
pub struct TupleUpload(String, Vec<u8>);

fn main() {}
