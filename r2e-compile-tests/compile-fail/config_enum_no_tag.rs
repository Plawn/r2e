use r2e::prelude::*;

#[derive(Clone, ConfigProperties)]
pub enum StorageConfig {
    S3,
    Filesystem,
}

fn main() {}
