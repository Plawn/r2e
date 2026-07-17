//! A bean method cannot be both `#[post_construct]` and `#[pre_destroy]` — the
//! two lifecycle hooks are mutually exclusive on one method.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Resource;

#[bean]
impl Resource {
    pub fn new() -> Self {
        Self
    }

    #[post_construct]
    #[pre_destroy]
    async fn lifecycle(&self) {}
}

fn main() {}
