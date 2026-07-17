//! A controller method cannot be both `#[post_construct]` and `#[pre_destroy]`
//! — the two lifecycle hooks are mutually exclusive on one method.

use r2e::prelude::*;

#[derive(Clone)]
pub struct AppState;

#[controller]
pub struct Svc {}

#[routes]
impl Svc {
    #[post_construct]
    #[pre_destroy]
    async fn lifecycle(&self) {}
}

fn main() {}
