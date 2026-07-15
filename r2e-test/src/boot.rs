//! Booting an [`App`] into a [`TestApp`].
//!
//! The [`App`] trait is the app's single declaration (`impl App for MyApp` in
//! the app's `lib.rs`). The harness pre-configures the builder — `test`
//! profile, pinned mocks, config overrides, a local [`TestJwt`] validator —
//! then runs `App::setup` + `App::build` to assemble the app exactly as
//! production does.

use std::sync::Arc;

use r2e_core::{App, AppBuilder, BootableApp};

use crate::{TestApp, TestJwt};

impl TestApp {
    /// Boot an [`App`] with test defaults:
    ///
    /// - active profile forced to `"test"` (so `load_config()` overlays
    ///   `application-test.yaml` when present),
    /// - a fresh [`TestJwt`] whose `JwtClaimsValidator`/`JwtValidator` are
    ///   **pinned** over whatever validator the app registers, so
    ///   [`as_user`](crate::TestRequest::as_user) mints accepted tokens with
    ///   no external IdP.
    ///
    /// Each boot runs `A::setup()` fresh, so every test gets its own
    /// environment.
    ///
    /// ```ignore
    /// let app = TestApp::boot::<MyApp>().await;
    /// app.get("/users").as_user("alice", &["admin"]).send().await.assert_ok();
    /// let service: UserService = app.bean();
    /// ```
    pub async fn boot<A: App>() -> Self {
        Self::boot_with::<A>(|b| b).await
    }

    /// [`boot`](Self::boot) with a builder pre-configuration hook — the place
    /// to pin mocks and patch config (Quarkus: `@InjectMock` /
    /// `@TestProfile`):
    ///
    /// ```ignore
    /// let app = TestApp::boot_with::<MyApp>(|b| {
    ///     b.override_bean(FakeMailer::new())
    ///         .override_config_value("app.greeting", "hello from tests")
    /// })
    /// .await;
    /// ```
    ///
    /// The hook runs after the harness defaults, so it may also re-pin the
    /// JWT validators or change the profile.
    pub async fn boot_with<A: App>(
        configure: impl FnOnce(AppBuilder) -> AppBuilder,
    ) -> Self {
        let jwt = TestJwt::new();
        let builder = AppBuilder::new()
            .with_profile("test")
            .override_bean(Arc::new(jwt.claims_validator()))
            .override_bean(Arc::new(jwt.validator()));
        let env = A::setup().await;
        let built = A::build(configure(builder), env).await;
        Self::from_bootable(built, Some(jwt)).await
    }

    /// Boot an [`App`] **without** the harness JWT wiring — for apps whose
    /// validator carries custom behaviour (role extractor, identity type)
    /// that the test wants to keep. The `test` profile is still forced.
    pub async fn boot_plain<A: App>(
        configure: impl FnOnce(AppBuilder) -> AppBuilder,
    ) -> Self {
        let builder = AppBuilder::new().with_profile("test");
        let env = A::setup().await;
        let built = A::build(configure(builder), env).await;
        Self::from_bootable(built, None).await
    }

    async fn from_bootable(built: impl BootableApp, jwt: Option<TestJwt>) -> Self {
        let bean_context = built.bean_context();
        let config = built.r2e_config();
        Self {
            // Run consumer registrations so `#[consumer]` methods, subscriber
            // beans, and EventBus bridges are live in tests, as in `serve()`.
            router: built.into_router_with_consumers().await,
            bean_context: Some(bean_context),
            config,
            jwt,
        }
    }
}
