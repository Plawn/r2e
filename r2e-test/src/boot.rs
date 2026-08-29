//! Booting an [`App`] into a [`TestApp`].
//!
//! The [`App`] trait is the app's single declaration (`impl App for MyApp` in
//! the app's `lib.rs`). The harness pre-configures the builder — `test`
//! profile, pinned mocks, config overrides, a local [`TestJwt`] validator —
//! then runs `App::setup` + `App::build` to assemble the app exactly as
//! production does.

use std::sync::Arc;

use r2e_core::{App, AppBuilder, BootError, BootableApp};

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

    /// [`boot`](Self::boot) without the panic: the boot error is returned so a
    /// test can assert *that* the app refuses to start (and on what).
    ///
    /// ```ignore
    /// let err = TestApp::try_boot::<MyApp>().await.unwrap_err();
    /// assert!(err.to_string().contains("DbPool"));
    /// ```
    pub async fn try_boot<A: App>() -> Result<Self, BootError> {
        Self::try_boot_with::<A>(|b| b).await
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
    pub async fn boot_with<A: App>(configure: impl FnOnce(AppBuilder) -> AppBuilder) -> Self {
        unwrap_boot::<A>(Self::try_boot_with::<A>(configure).await)
    }

    /// [`boot_with`](Self::boot_with) without the panic — see
    /// [`try_boot`](Self::try_boot).
    pub async fn try_boot_with<A: App>(
        configure: impl FnOnce(AppBuilder) -> AppBuilder,
    ) -> Result<Self, BootError> {
        let jwt = TestJwt::new();
        let builder = AppBuilder::new()
            .with_profile("test")
            .override_bean(Arc::new(jwt.claims_validator()))
            .override_bean(Arc::new(jwt.validator()));
        let env = A::setup().await?;
        let built = A::build(configure(builder), env).await?;
        Self::from_bootable(built, Some(jwt)).await
    }

    /// Boot an [`App`] **without** the harness JWT wiring — for apps whose
    /// validator carries custom behaviour (role extractor, identity type)
    /// that the test wants to keep. The `test` profile is still forced.
    pub async fn boot_plain<A: App>(configure: impl FnOnce(AppBuilder) -> AppBuilder) -> Self {
        unwrap_boot::<A>(Self::try_boot_plain::<A>(configure).await)
    }

    /// [`boot_plain`](Self::boot_plain) without the panic — see
    /// [`try_boot`](Self::try_boot).
    pub async fn try_boot_plain<A: App>(
        configure: impl FnOnce(AppBuilder) -> AppBuilder,
    ) -> Result<Self, BootError> {
        let builder = AppBuilder::new().with_profile("test");
        let env = A::setup().await?;
        let built = A::build(configure(builder), env).await?;
        Self::from_bootable(built, None).await
    }

    /// Start the assembled app through the production lifecycle.
    ///
    /// `start_in_process()` is the same startup phase `serve()` runs —
    /// controller `#[post_construct]`, consumer registrations (`#[consumer]`
    /// methods, subscriber beans, EventBus bridges), bean/controller
    /// `#[on_start]`, then the builder's `on_start` closures, which is what
    /// starts `spawn_service` / `#[derive(BackgroundService)]` tasks. The
    /// returned [`RunningApp`](r2e_core::RunningApp) is kept so
    /// [`TestApp::shutdown`] can run the matching shutdown sequence under the
    /// app's own budgets.
    ///
    /// The one production phase it skips is the plugin **serve hooks** (they
    /// bind ports: separate-port gRPC, MCP, and they start the scheduler
    /// driver) — see `PreparedApp::start_in_process`.
    async fn from_bootable(
        built: impl BootableApp,
        jwt: Option<TestJwt>,
    ) -> Result<Self, BootError> {
        let bean_context = built.bean_context();
        let config = built.r2e_config();
        // The production startup phase runs here: controller
        // `#[post_construct]`, consumer registrations, `#[on_start]`, then the
        // builder's `on_start` closures. An `Err` from any of them is a boot
        // failure like any other — returned to `try_boot*` with the failing
        // phase named, rendered by `unwrap_boot` for `boot*`.
        let running = built.start_in_process().await?;
        Ok(Self {
            router: running.router().clone(),
            bean_context: Some(bean_context),
            config,
            jwt,
            running: Some(running),
        })
    }
}

/// Turn a boot failure into an attributable test failure.
///
/// The panic message names the app type and the whole error chain, so the
/// harness never swallows the cause the way a `process::exit` in `setup`
/// would (that kills the test binary, failing every test in it with no
/// attribution).
fn unwrap_boot<A: App>(result: Result<TestApp, BootError>) -> TestApp {
    match result {
        Ok(app) => app,
        Err(err) => {
            let mut chain = err.to_string();
            let mut source = std::error::Error::source(err.as_ref());
            while let Some(cause) = source {
                chain.push_str("\n  caused by: ");
                chain.push_str(&cause.to_string());
                source = std::error::Error::source(cause);
            }
            panic!(
                "TestApp::boot::<{}>() failed: {chain}",
                std::any::type_name::<A>()
            );
        }
    }
}
