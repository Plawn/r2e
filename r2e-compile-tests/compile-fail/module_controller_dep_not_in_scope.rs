//! A module controller injecting a bean outside the module's scope
//! (neither provided nor imported) must be rejected at `register_module`.

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

#[controller(path = "/bad")]
pub struct BadController {
    #[inject]
    pool: Pool, // not in the module's scope
}

#[routes]
impl BadController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        let _ = &self.pool;
        "bad"
    }
}

pub struct BadModule;

impl FeatureModule for BadModule {
    type Providers = TCons<Svc, TNil>;
    type Controllers = (BadController,);
    type Exports = TCons<Svc, TNil>;
    type Imports = TNil;
}

fn main() {
    let _ = r2e::AppBuilder::new().register_module::<BadModule>();
}
