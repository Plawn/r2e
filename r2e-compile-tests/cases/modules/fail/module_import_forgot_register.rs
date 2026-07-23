//! Importing a module by name only *requires* its exports — it does not
//! register that module. Forgetting to `.register_module` the imported module
//! leaves its export unprovided, a missing-provision error at `build_state()`.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Pool;

#[derive(Clone)]
pub struct UserSvc {
    pool: Pool,
}

#[bean]
impl UserSvc {
    fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[module(providers(UserSvc), exports(UserSvc), imports(Pool))]
pub struct UserModule;

#[derive(Clone)]
pub struct OrderSvc {
    users: UserSvc,
}

#[bean]
impl OrderSvc {
    fn new(users: UserSvc) -> Self {
        Self { users }
    }
}

#[module(
    providers(OrderSvc),
    exports(OrderSvc),
    imports(module(UserModule))
)]
pub struct OrderModule;

fn main() {
    let _ = async {
        r2e::AppBuilder::new()
            .provide(Pool)
            // UserModule is imported by OrderModule but never registered.
            .register_module::<OrderModule>()
            .build_state()
            .await
    };
}
