//! A module's non-exported provider is invisible outside the module: an
//! app-level controller injecting it must be rejected at
//! `register_controller` (the bean is absent from the application state).

use r2e::prelude::*;
use r2e::type_list::{TCons, TNil};

#[derive(Clone)]
pub struct Secret;

#[bean]
impl Secret {
    fn new() -> Self {
        Self
    }
}

#[derive(Clone)]
pub struct Api;

#[bean]
impl Api {
    fn new() -> Self {
        Self
    }
}

pub struct UserModule;

impl FeatureModule for UserModule {
    type Providers = TCons<Secret, TCons<Api, TNil>>;
    type Controllers = ();
    type Exports = TCons<Api, TNil>; // Secret stays private
    type Imports = TNil; type RequiredPlugins = ();
}

#[controller(path = "/spy")]
pub struct SpyController {
    #[inject]
    secret: Secret, // private to UserModule — not in the state
}

#[routes]
impl SpyController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        let _ = &self.secret;
        "spy"
    }
}

fn main() {
    let _ = async {
        r2e::AppBuilder::new()
            .register_module::<UserModule>()
            .build_state()
            .await
            .register_controller::<SpyController>()
    };
}
