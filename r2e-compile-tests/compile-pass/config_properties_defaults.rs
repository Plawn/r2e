use r2e::prelude::*;

#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "server")]
pub struct ServerConfig {
    pub host: String,
    #[config(default = 8080)]
    pub port: i64,
    #[config(default = true)]
    pub tls: bool,
}

fn main() {}
