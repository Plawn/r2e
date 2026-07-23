//! `imports(module(...))` composes one module on another's exports without
//! restating the exported bean types, mixed freely with plain bean imports.

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
    pool: Pool,
}

#[bean]
impl OrderSvc {
    fn new(users: UserSvc, pool: Pool) -> Self {
        Self { users, pool }
    }
}

#[controller(path = "/orders")]
pub struct OrderController {
    #[inject]
    svc: OrderSvc,
}

#[routes]
impl OrderController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        let _ = &self.svc;
        "ok"
    }
}

// `module(UserModule)` appends `UserSvc`; `Pool` is a plain bean import.
#[module(
    providers(OrderSvc),
    controllers(OrderController),
    exports(OrderSvc),
    imports(Pool, module(UserModule))
)]
pub struct OrderModule;

fn main() {
    let _ = async {
        r2e::AppBuilder::new()
            .provide(Pool)
            .register_module::<UserModule>()
            .register_module::<OrderModule>()
            .build_state()
            .await
            .build()
    };
}
