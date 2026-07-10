//! Booting an application blueprint into a [`TestApp`].
//!
//! The blueprint is the app's single assembly function (usually
//! `pub async fn app(b: AppBuilder) -> impl BootableApp` in the app's
//! `lib.rs`). The harness pre-configures the builder — `test` profile,
//! pinned mocks, config overrides, a local [`TestJwt`] validator — and hands
//! it to the blueprint, which assembles the app exactly as production does.

use std::future::Future;
use std::sync::Arc;

use r2e_core::{AppBuilder, BootableApp};

use crate::{TestApp, TestJwt};

impl TestApp {
    /// Boot an application blueprint with test defaults:
    ///
    /// - active profile forced to `"test"` (so `load_config()` overlays
    ///   `application-test.yaml` when present),
    /// - a fresh [`TestJwt`] whose `JwtClaimsValidator`/`JwtValidator` are
    ///   **pinned** over whatever validator the app registers, so
    ///   [`as_user`](crate::TestRequest::as_user) mints accepted tokens with
    ///   no external IdP.
    ///
    /// ```ignore
    /// let app = TestApp::boot(example_app::app).await;
    /// app.get("/users").as_user("alice", &["admin"]).send().await.assert_ok();
    /// let service: UserService = app.bean();
    /// ```
    pub async fn boot<F, Fut, B>(blueprint: F) -> Self
    where
        F: FnOnce(AppBuilder) -> Fut,
        Fut: Future<Output = B>,
        B: BootableApp,
    {
        Self::boot_with(blueprint, |b| b).await
    }

    /// [`boot`](Self::boot) with a builder pre-configuration hook — the place
    /// to pin mocks and patch config (Quarkus: `@InjectMock` /
    /// `@TestProfile`):
    ///
    /// ```ignore
    /// let app = TestApp::boot_with(example_app::app, |b| {
    ///     b.override_bean(FakeMailer::new())
    ///         .override_config_value("app.greeting", "hello from tests")
    /// })
    /// .await;
    /// ```
    ///
    /// The hook runs after the harness defaults, so it may also re-pin the
    /// JWT validators or change the profile.
    pub async fn boot_with<F, Fut, B>(
        blueprint: F,
        configure: impl FnOnce(AppBuilder) -> AppBuilder,
    ) -> Self
    where
        F: FnOnce(AppBuilder) -> Fut,
        Fut: Future<Output = B>,
        B: BootableApp,
    {
        let jwt = TestJwt::new();
        let builder = AppBuilder::new()
            .with_profile("test")
            .override_bean(Arc::new(jwt.claims_validator()))
            .override_bean(Arc::new(jwt.validator()));
        let built = blueprint(configure(builder)).await;
        Self::from_bootable(built, Some(jwt))
    }

    /// Boot a blueprint **without** the harness JWT wiring — for apps whose
    /// validator carries custom behaviour (role extractor, identity type)
    /// that the test wants to keep. The `test` profile is still forced.
    pub async fn boot_plain<F, Fut, B>(
        blueprint: F,
        configure: impl FnOnce(AppBuilder) -> AppBuilder,
    ) -> Self
    where
        F: FnOnce(AppBuilder) -> Fut,
        Fut: Future<Output = B>,
        B: BootableApp,
    {
        let builder = AppBuilder::new().with_profile("test");
        let built = blueprint(configure(builder)).await;
        Self::from_bootable(built, None)
    }

    fn from_bootable(built: impl BootableApp, jwt: Option<TestJwt>) -> Self {
        let bean_context = built.bean_context();
        let config = built.r2e_config();
        Self {
            router: built.into_router(),
            bean_context: Some(bean_context),
            config,
            jwt,
        }
    }
}
