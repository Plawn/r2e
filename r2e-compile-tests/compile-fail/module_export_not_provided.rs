//! A module exporting a type none of its providers outputs must be rejected
//! at `register_module` (Exports ⊆ Provided).

use r2e::prelude::*;
use r2e::type_list::{TCons, TNil};

#[derive(Clone)]
pub struct Pool;

#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    fn new() -> Self {
        Self
    }
}

pub struct BadModule;

impl FeatureModule for BadModule {
    type Providers = TCons<Svc, TNil>;
    type Controllers = ();
    type Exports = TCons<Pool, TNil>; // Pool is not provided by this module
    type Imports = TNil; type RequiredPlugins = ();
}

fn main() {
    let _ = r2e::AppBuilder::new().register_module::<BadModule>();
}
