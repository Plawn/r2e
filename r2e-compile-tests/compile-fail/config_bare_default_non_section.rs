use r2e::prelude::*;

#[derive(Clone, ConfigProperties)]
pub struct MyConfig {
    #[config(default)]
    pub name: String,
}

fn main() {}
