//! Test resources: what a test needs running *before* the app boots.
//!
//! A [`TestResource`] starts something the app depends on — a database, a
//! broker, a stub server — and says which config keys point the app at it
//! (Quarkus: `QuarkusTestResourceLifecycleManager`). Declared on the test
//! attribute, it is started before the boot, wired into the builder, and kept
//! alive for as long as the [`TestApp`]:
//!
//! ```ignore
//! struct Db(DevPostgres);
//!
//! impl TestResource for Db {
//!     async fn start() -> Self {
//!         Db(DevPostgres::shared().await.fresh_database().await)
//!     }
//!
//!     fn config(&self) -> Vec<(String, String)> {
//!         vec![("datasource.url".into(), self.0.url().into())]
//!     }
//! }
//!
//! #[r2e::test(app = MyApp, resources(Db))]
//! async fn lists_articles(app: TestApp) {
//!     app.get("/articles").send().await.assert_ok();
//! }
//! ```
//!
//! The trait knows nothing about containers: `r2e-devservices` is the usual
//! way to implement `start`, not a requirement.

use std::any::{Any, TypeId};
use std::future::Future;

use r2e_core::AppBuilder;

use crate::TestApp;

/// Something a test needs running before the app boots, and the config that
/// points the app at it.
///
/// See the [module docs](self) for the whole picture.
pub trait TestResource: Send + Sync + Sized + 'static {
    /// Start the resource. Runs before the app boots, once per boot — once per
    /// test under `#[r2e::test]`, once per suite under `#[r2e::test_suite]`.
    ///
    /// Share what is expensive (`DevPostgres::shared()` starts one container
    /// for the whole session) and isolate what is cheap (a
    /// `fresh_database()` per start).
    ///
    /// A resource that cannot start should panic: there is no app to boot
    /// without it, and the panic message is the test failure.
    fn start() -> impl Future<Output = Self> + Send;

    /// The config keys this resource sets on the app being booted, applied
    /// with [`override_config_value`](AppBuilder::override_config_value)
    /// before the test's own `with = ...` hook — which therefore wins on a
    /// key both set.
    ///
    /// Values are strings, converted on read like an environment variable's.
    fn config(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// The started resources of one boot.
///
/// `#[r2e::test(app = ..., resources(A, B))]` builds one of these; reach for
/// it directly when booting by hand:
///
/// ```ignore
/// let resources = TestResources::new().start::<Db>().await;
/// let app = TestApp::boot_with::<MyApp>(resources.configure(|b| b))
///     .await
///     .with_resources(resources);
/// ```
#[derive(Default)]
pub struct TestResources {
    config: Vec<(String, String)>,
    started: Vec<(TypeId, Box<dyn Any + Send + Sync>)>,
}

impl TestResources {
    /// No resource yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start `R` and record the config it asks for. Resources start in the
    /// order they are declared, and a later one wins on a key two of them set.
    ///
    /// # Panics
    ///
    /// Panics if `R` was already started here: a resource is looked up by
    /// type, so a second one would be unreachable.
    pub async fn start<R: TestResource>(mut self) -> Self {
        assert!(
            self.get::<R>().is_none(),
            "the test resource `{}` is declared twice",
            std::any::type_name::<R>()
        );
        let resource = R::start().await;
        self.config.extend(resource.config());
        self.started.push((TypeId::of::<R>(), Box::new(resource)));
        self
    }

    /// A builder hook applying the resources' config, then `then` — the shape
    /// [`TestApp::boot_with`] takes.
    pub fn configure(
        &self,
        then: impl FnOnce(AppBuilder) -> AppBuilder,
    ) -> impl FnOnce(AppBuilder) -> AppBuilder {
        let config = self.config.clone();
        move |mut builder| {
            for (key, value) in config {
                builder = builder.override_config_value(key, value);
            }
            then(builder)
        }
    }

    /// The started resource of type `R`, if any.
    pub fn get<R: TestResource>(&self) -> Option<&R> {
        self.started
            .iter()
            .find(|(id, _)| *id == TypeId::of::<R>())
            .and_then(|(_, resource)| resource.downcast_ref())
    }
}

impl TestApp {
    /// Hand the resources this app was booted on over to it: they are dropped
    /// with the app — after it has stopped using them — and readable through
    /// [`resource`](Self::resource).
    pub fn with_resources(mut self, resources: TestResources) -> Self {
        self.resources = resources;
        self
    }

    /// A resource this app was booted on, by type — to seed or inspect what
    /// the app talks to:
    ///
    /// ```ignore
    /// #[r2e::test(app = MyApp, resources(Db))]
    /// async fn sees_seeded_rows(app: TestApp) {
    ///     seed(app.resource::<Db>().0.url()).await;
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the app was not booted on an `R`.
    pub fn resource<R: TestResource>(&self) -> &R {
        self.resources.get::<R>().unwrap_or_else(|| {
            panic!(
                "the test app was not booted on a `{}` resource — declare it with \
                 `#[r2e::test(app = ..., resources({0}))]`",
                std::any::type_name::<R>()
            )
        })
    }
}
