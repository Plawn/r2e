use r2e::prelude::*;

#[derive(Clone, ConfigProperties)]
#[config(tag = "backend")]
pub enum StorageConfig {
    S3 { bucket: String },
    Filesystem,
}

fn main() {}
