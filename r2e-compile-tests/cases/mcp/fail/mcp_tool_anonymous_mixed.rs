//! `#[anonymous]` is an HTTP-pipeline concept: on an MCP member it is
//! rejected even when the `#[routes]` impl also carries HTTP routes (where it
//! is accepted).

use r2e::prelude::*;

#[controller(path = "/calc")]
pub struct Calc {
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl Calc {
    #[get("/health")]
    #[anonymous]
    async fn health(&self) -> &'static str {
        "ok"
    }

    #[tool]
    #[anonymous]
    async fn whoami(&self) -> String {
        self.user.sub.clone()
    }
}

fn main() {}
