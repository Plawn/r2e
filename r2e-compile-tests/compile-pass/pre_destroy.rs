//! `#[pre_destroy]` disposal hooks compile on both `#[bean]` impls and
//! `#[routes]` controller impls (parity with `#[post_construct]`).

use r2e::prelude::*;

#[derive(Clone)]
pub struct Resource;

#[bean]
impl Resource {
    pub fn new() -> Self {
        Self
    }

    #[pre_destroy]
    async fn close(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }

    #[pre_destroy]
    fn sync_close(&self) {}
}

#[controller(path = "/svc")]
pub struct Svc {
    #[inject]
    _res: Resource,
}

#[routes]
impl Svc {
    #[get("/")]
    async fn root(&self) -> &'static str {
        "ok"
    }

    #[pre_destroy]
    async fn on_shutdown(&self) {}
}

fn main() {}
