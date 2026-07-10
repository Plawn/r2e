use r2e::prelude::*;

#[derive(Clone, ConfigProperties)]
pub struct TlsConfig {
    pub cert: Option<String>,
}

#[derive(Clone, ConfigProperties)]
pub struct MyConfig {
    #[config(section, default)]
    pub tls: Option<TlsConfig>,
}

fn main() {}
