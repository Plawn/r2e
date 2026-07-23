use r2e::prelude::*;

#[derive(Clone, ConfigProperties)]
pub struct MyConfig {
    #[config(skip, default = "x")]
    pub token: String,
}

fn main() {}
