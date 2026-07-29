//! `#[live_config("key")]` field form on both hosts: a `#[derive(Bean)]` struct
//! and a `#[controller]` struct. Both resolve the handle from the
//! `LiveConfigRegistry` bean at construction; `live_config` must be a
//! registered helper attribute on the derive so it never leaks to rustc.
use r2e::prelude::*;

#[derive(Clone, Bean)]
pub struct LiveService {
    #[live_config("db.url")]
    url: LiveConfig<String>,
    #[config("app.name")]
    name: String,
}

#[controller(path = "/live")]
pub struct LiveController {
    #[live_config("app.banner")]
    banner: LiveConfig<String>,
    #[inject]
    service: LiveService,
}

#[routes]
impl LiveController {
    #[get("/")]
    async fn index(&self) -> String {
        format!(
            "{} {}",
            self.banner.get().unwrap_or_default(),
            self.service.name
        )
    }
}

fn main() {}
