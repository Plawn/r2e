use r2e::prelude::*;

#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app")]
pub struct AppConfig {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
}

fn main() {}
