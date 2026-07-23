use r2e::prelude::*;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Cacheable)]
pub struct UserList {
    pub users: Vec<String>,
    pub total: usize,
}

#[derive(Serialize, Deserialize, Cacheable)]
pub struct CachedConfig {
    pub key: String,
    pub value: String,
}

fn main() {}
