//! `#[routes]` emits two brand-new impl blocks in place of the one the user
//! wrote, so it must carry over what it does not consume (task #985):
//!
//!   * impl-block attributes — `#![deny(unused_variables)]` below turns the
//!     `#[allow(unused_variables)]` on the impl into a load-bearing assertion,
//!     on the façade impl (route methods) and on the core impl alike;
//!   * non-`fn` impl items — associated consts used to be swallowed by the
//!     method-classification loop, turning `ItemController::PAGE_SIZE` into a
//!     bewildering E0599.
#![deny(unused_variables)]

use r2e::prelude::*;

#[derive(Clone)]
pub struct Repo;

#[controller(path = "/items")]
pub struct ItemController {
    #[inject]
    repo: Repo,
}

/// Item endpoints.
#[allow(unused_variables)]
#[routes]
impl ItemController {
    const PAGE_SIZE: usize = 50;

    #[get("/")]
    async fn list(&self) -> String {
        let unused_on_the_facade = 1u8;
        format!("{} rows", ItemController::PAGE_SIZE)
    }

    // Not a route: lands on the CORE impl, so the allow must reach that one too.
    fn helper(&self) -> String {
        let unused_on_the_core = 2u8;
        format!("{}", Self::PAGE_SIZE)
    }
}

fn main() {
    let _ = ItemController::PAGE_SIZE;
}
