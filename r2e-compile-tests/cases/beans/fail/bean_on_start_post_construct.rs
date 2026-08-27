//! A bean method cannot be both `#[on_start]` and `#[post_construct]` — the
//! startup observer runs in a later phase than the construction hook.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Resource;

#[bean]
impl Resource {
    pub fn new() -> Self {
        Self
    }

    #[post_construct]
    #[on_start]
    async fn lifecycle(&self) {}
}

fn main() {}
