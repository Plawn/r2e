use r2e::prelude::*;

#[derive(ConfigProperties, Clone, Debug)]
pub struct DbConfig {
    pub url: String,
    pub max_connections: i64,
    pub debug: bool,
}

fn main() {}
