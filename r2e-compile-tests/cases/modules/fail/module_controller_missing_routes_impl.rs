//! A type placed in a module's `Controllers` tuple without a `#[routes]`
//! block (so no `EndpointDeps` impl) must be rejected at `register_module`
//! with the "has no `#[routes]` impl" diagnostic.

use r2e::prelude::*;
use r2e::type_list::{TCons, TNil};

#[derive(Clone)]
pub struct Svc;

#[bean]
impl Svc {
    fn new() -> Self {
        Self
    }
}

/// Not a controller: no `#[controller]`, no `#[routes]`.
pub struct PlainService;

pub struct BadModule;

impl FeatureModule for BadModule {
    type Providers = TCons<Svc, TNil>;
    type Controllers = (PlainService,);
    type Exports = TCons<Svc, TNil>;
    type Imports = TNil; type RequiredPlugins = ();
}

fn main() {
    let _ = r2e::AppBuilder::new().register_module::<BadModule>();
}
