//! A well-formed feature module: private + exported providers, an import
//! satisfied by the app, module controllers (one on a private bean), and an
//! app-level controller consuming the export — all through the `r2e` facade.

use r2e::prelude::*;
use r2e::type_list::{TCons, TNil};

#[derive(Clone)]
pub struct Pool;

#[derive(Clone)]
pub struct Repo {
    pool: Pool,
}

#[bean]
impl Repo {
    fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[derive(Clone)]
pub struct Svc {
    repo: Repo,
}

#[bean]
impl Svc {
    fn new(repo: Repo) -> Self {
        Self { repo }
    }
}

#[controller(path = "/users")]
pub struct UserController {
    #[inject]
    svc: Svc,
    #[inject]
    repo: Repo, // private module bean — fine inside the module
}

#[routes]
impl UserController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        let _ = (&self.svc, &self.repo);
        "ok"
    }
}

pub struct UserModule;

impl FeatureModule for UserModule {
    type Providers = TCons<Repo, TCons<Svc, TNil>>;
    type Controllers = (UserController,);
    type Exports = TCons<Svc, TNil>;
    type Imports = TCons<Pool, TNil>;
}

#[controller(path = "/app")]
pub struct AppController {
    #[inject]
    svc: Svc, // exported — visible to the app
}

#[routes]
impl AppController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        let _ = &self.svc;
        "ok"
    }
}

fn main() {
    let _ = async {
        r2e::AppBuilder::new()
            .provide(Pool)
            .register_module::<UserModule>()
            .build_state()
            .await
            .register_controller::<AppController>()
            .build()
    };
}
