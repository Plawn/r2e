//! Two `#[config(section)]` fields of the SAME type both call
//! `registry.provide::<DatabaseSettings>()` in the generated
//! `register_children`, so the last one silently wins and every
//! `#[inject] DatabaseSettings` reads whichever that was. R2E has no bean
//! qualifiers (decisions log: newtypes are the sanctioned answer), so the
//! declaration is the error.

use r2e::prelude::*;

#[derive(Clone, ConfigProperties)]
pub struct DatabaseSettings {
    pub url: String,
}

#[derive(Clone, ConfigProperties)]
pub struct Settings {
    #[config(section)]
    pub database: DatabaseSettings,
    #[config(section)]
    pub run_database: DatabaseSettings,
}

fn main() {}
