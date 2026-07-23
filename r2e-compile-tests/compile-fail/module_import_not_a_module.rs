//! `imports(module(X))` where `X` does not implement `FeatureModule` must be
//! rejected — the generated `<X as FeatureModule>::Exports` projection fails
//! the `FeatureModule` bound.

use r2e::prelude::*;

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

/// Not a module — just a plain bean type.
#[derive(Clone)]
pub struct NotAModule;

#[module(
    providers(Svc),
    exports(Svc),
    imports(module(NotAModule))
)]
pub struct BadModule;

fn main() {
    let _ = async {
        r2e::AppBuilder::new()
            .provide(Pool)
            .register_module::<BadModule>()
            .build_state()
            .await
    };
}
