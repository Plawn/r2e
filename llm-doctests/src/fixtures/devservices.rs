//! Scaffolding for `llm/devservices.md`.

use r2e::prelude::*;

pub use r2e::r2e_openfga::{FgaClient, FgaObject};

/// The typed authorization model the OpenFGA block seeds a tuple into —
/// `model!(pub mod authz = "fga/model.fga")`, see `llm/openfga.md`.
pub use crate::fixtures::openfga::authz;

/// What `authz::user::id("alice")` returns.
pub type UserRef = FgaObject<authz::user::Ty>;

/// What `authz::document::id("readme")` returns.
pub type DocRef = FgaObject<authz::document::Ty>;

/// A store name no other test uses, so tests sharing the session container
/// stay isolated.
pub fn unique_store_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("test-store-{}", N.fetch_add(1, Ordering::Relaxed))
}

/// The application under test, as `llm/testing.md` declares it.
pub struct MyApp;

impl App for MyApp {
    type Env = ();

    async fn setup() -> Result<Self::Env, BootError> {
        Ok(())
    }

    async fn build(b: AppBuilder, _env: Self::Env) -> Result<impl BootableApp, BootError> {
        Ok(b.build_state().await)
    }
}
