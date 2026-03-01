use r2e::prelude::*;

#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "oidc")]
pub struct OidcConfig {
    pub issuer: Option<String>,
    #[config(key = "jwks.url")]
    pub jwks_url: Option<String>,
    #[config(default = "my-app")]
    pub audience: String,
    #[config(key = "client.id", default = "my-app")]
    pub client_id: String,
}

fn main() {}
