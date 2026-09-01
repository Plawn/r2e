//! Scaffolding for `llm/testing.md`.

use r2e::prelude::*;

/// The application under test. `#[r2e::test(app = my_app::MyApp)]` names it
/// through the crate that declares it.
pub use crate::fixtures::devservices::MyApp;

/// The `my_app` crate/module the `app = my_app::MyApp` paths refer to.
pub mod my_app {
    pub use crate::fixtures::devservices::MyApp;
}

/// A bean the app provides and the tests inject.
#[derive(Clone, Default)]
pub struct UserService;

#[bean]
impl UserService {
    pub fn new() -> Self {
        Self
    }

    /// Called from `#[before_each]`.
    pub async fn clear(&self) {}
}

/// The mocks pinned with `override_bean`.
#[derive(Clone, Default)]
pub struct MockMailer;

impl MockMailer {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[derive(Clone, Default)]
pub struct MockUsers;

impl MockUsers {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// The already-constructed mock handed to `boot_with_env`'s closure.
#[allow(non_upper_case_globals)]
pub const mock: MockMailer = MockMailer;

/// Seeds reference data once, inside a `SharedEnv::with` initialiser.
pub async fn seed_reference_data(_env: &()) -> Result<(), BootError> {
    Ok(())
}
