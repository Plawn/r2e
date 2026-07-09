//! A module provider depending on a bean the module neither provides nor
//! imports must be rejected at `register_module` (closed-subgraph check).

use r2e::prelude::*;
use r2e::type_list::{TCons, TNil};

#[derive(Clone)]
pub struct Pool;

#[derive(Clone)]
pub struct Svc {
    pool: Pool,
}

#[bean]
impl Svc {
    fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

pub struct BadModule;

impl FeatureModule for BadModule {
    type Providers = TCons<Svc, TNil>;
    type Controllers = ();
    type Exports = TCons<Svc, TNil>;
    type Imports = TNil; // Pool is neither provided nor imported
}

fn main() {
    let _ = r2e::AppBuilder::new().register_module::<BadModule>();
}
