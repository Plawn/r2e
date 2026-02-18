use r2e::prelude::*;

#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "db")]
pub struct DbConfig {
    pub url: String,
    pub max_connections: i64,
    pub debug: bool,
}

fn main() {}
