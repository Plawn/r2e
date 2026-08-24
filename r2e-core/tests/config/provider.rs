use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use r2e_core::rt::CancelToken;

use r2e_core::beans::Producer;
use r2e_core::config::{
    ConfigError, ConfigKeyKind, ConfigProvider, ConfigProviderContext, ConfigUpdateSink,
    ConfigValue, ConfigWatchContext, LiveConfig, LiveConfigRegistry, R2eConfig,
};
use r2e_core::{AppBuilder, BeanAccess};

#[r2e_macros::producer]
fn produce_db_url(#[live_config("db.url")] url: LiveConfig<String>) -> LiveConfig<String> {
    url
}

#[derive(Clone)]
struct StaticProvider {
    key: &'static str,
    value: &'static str,
}

impl ConfigProvider for StaticProvider {
    fn load(
        &self,
        config: &mut R2eConfig,
        _ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError> {
        config.set(self.key, ConfigValue::String(self.value.to_string()));
        Ok(())
    }
}

#[derive(Clone)]
struct WatchProvider {
    key: &'static str,
    value: &'static str,
}

impl ConfigProvider for WatchProvider {
    fn load(
        &self,
        config: &mut R2eConfig,
        _ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError> {
        config.set(self.key, ConfigValue::String("initial".to_string()));
        Ok(())
    }

    fn watch(
        self: Arc<Self>,
        _ctx: ConfigWatchContext,
        sink: ConfigUpdateSink,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConfigError>> + Send + 'static>> {
        Box::pin(async move {
            sink.set(self.key, self.value.to_string());
            Ok(())
        })
    }
}

#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
struct ProviderConfig {
    app_name: String,
}

#[test]
fn live_config_handle_reads_updates() {
    let registry = LiveConfigRegistry::new();
    registry.set("db.url", "postgres://first");

    let handle: LiveConfig<String> = registry.live_config("db.url");
    assert_eq!(handle.get().unwrap(), "postgres://first");

    registry.set("db.url", "postgres://second");
    assert_eq!(handle.get().unwrap(), "postgres://second");
    assert_eq!(handle.snapshot().version(), 2);
}

#[tokio::test]
async fn provider_values_are_visible_to_typed_config_and_live_handles() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .with_config_provider(StaticProvider {
            key: "app_name",
            value: "from-provider",
        })
        .load_config::<ProviderConfig>()
        .build_state()
        .await;

    assert_eq!(
        app.state().get::<ProviderConfig>().app_name,
        "from-provider"
    );
    let registry = app.state().get::<LiveConfigRegistry>();
    assert_eq!(registry.get::<String>("app_name").unwrap(), "from-provider");
}

#[tokio::test]
async fn override_config_value_pins_runtime_live_updates() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .with_config_provider(WatchProvider {
            key: "db.url",
            value: "provider-update",
        })
        .override_config_value("db.url", "test-override")
        .load_config::<()>()
        .build_state()
        .await;

    let registry = app.state().get::<LiveConfigRegistry>();
    let sink = ConfigUpdateSink::new(registry.clone());
    assert!(!sink.set("db.url", "provider-update"));
    assert_eq!(registry.get::<String>("db.url").unwrap(), "test-override");
}

/// `override_config_value` is documented as order-agnostic: called *after*
/// `load_config` it must patch the live registry too, not just `R2eConfig` —
/// otherwise `#[live_config]` handles keep reading the pre-override value.
#[tokio::test]
async fn late_override_config_value_reaches_live_registry() {
    let mut config = R2eConfig::empty();
    config.set("db.url", ConfigValue::String("postgres://boot".to_string()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .override_config_value("db.url", "postgres://late-override")
        .build_state()
        .await;

    let registry = app.state().get::<LiveConfigRegistry>();
    assert_eq!(
        registry.get::<String>("db.url").unwrap(),
        "postgres://late-override"
    );
    // The raw config bean stays in sync with the live slot.
    assert_eq!(
        app.state()
            .get::<R2eConfig>()
            .try_get::<String>("db.url")
            .unwrap(),
        "postgres://late-override"
    );
}

/// A late override must also be pinned: a provider's runtime watch may not
/// overwrite it (same guarantee the pre-`load_config` path gets).
#[tokio::test]
async fn late_override_config_value_pins_runtime_live_updates() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .with_config_provider(WatchProvider {
            key: "db.url",
            value: "provider-update",
        })
        .load_config::<()>()
        .override_config_value("db.url", "test-override")
        .build_state()
        .await;

    let registry = app.state().get::<LiveConfigRegistry>();
    let sink = ConfigUpdateSink::new(registry.clone());
    assert!(!sink.set("db.url", "provider-update"));
    assert_eq!(registry.get::<String>("db.url").unwrap(), "test-override");
}

/// End-to-end: a producer's `#[live_config]` handle resolves the late override.
#[tokio::test]
async fn late_override_config_value_is_visible_to_producer_handle() {
    let mut config = R2eConfig::empty();
    config.set("db.url", ConfigValue::String("postgres://boot".to_string()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .override_config_value("db.url", "postgres://late-override")
        .register::<ProduceDbUrl>()
        .build_state()
        .await;

    let handle = app.state().get::<LiveConfig<String>>();
    assert_eq!(handle.get().unwrap(), "postgres://late-override");
}

#[tokio::test]
async fn producer_live_config_param_gets_runtime_handle() {
    let mut config = R2eConfig::empty();
    config.set("db.url", ConfigValue::String("postgres://boot".to_string()));

    let app = AppBuilder::new()
        .override_config(config)
        .load_config::<()>()
        .register::<ProduceDbUrl>()
        .build_state()
        .await;

    let handle = app.state().get::<LiveConfig<String>>();
    assert_eq!(handle.get().unwrap(), "postgres://boot");
}

/// A `#[live_config("key")]` param is declared in `config_keys()` with kind
/// `Live`: an absent key never fails startup validation (the handle's `get()`
/// returns a `Result`), and the key stays out of the producer's dev-reload
/// fingerprint (freshness arrives by push, not by rebuild).
#[test]
fn live_config_param_is_subscribed_not_copied() {
    let keys = <ProduceDbUrl as Producer>::config_keys();
    let entry = keys
        .iter()
        .find(|(key, _, _)| *key == "db.url")
        .expect("live_config key must appear in config_keys()");
    assert_eq!(entry.2, ConfigKeyKind::Live);
    assert!(
        !entry.2.is_required(),
        "live_config keys must not be presence-validated"
    );
    assert!(
        !entry.2.is_fingerprinted(),
        "live_config keys must not rebuild the producer when edited"
    );
}

/// The absent-at-boot case the `Live` kind exists for: a producer
/// with a `#[live_config]` param whose key is missing from the config still
/// builds, and the handle simply reports the missing key at `get()` time.
#[tokio::test]
async fn producer_live_config_param_tolerates_missing_key_at_boot() {
    let app = AppBuilder::new()
        .override_config(R2eConfig::empty())
        .load_config::<()>()
        .register::<ProduceDbUrl>()
        .build_state()
        .await;

    let handle = app.state().get::<LiveConfig<String>>();
    assert!(handle.get().is_err());

    // Once a provider pushes the value at runtime, the same handle sees it.
    let registry = app.state().get::<LiveConfigRegistry>();
    assert!(registry.set("db.url", "postgres://late"));
    assert_eq!(handle.get().unwrap(), "postgres://late");
}

// ── Watch supervision ──────────────────────────────────────────────────────

/// Fails `fail_times` times, then pushes its value and returns `Ok(())`.
struct FlakyWatchProvider {
    key: &'static str,
    value: &'static str,
    attempts: Arc<AtomicUsize>,
    fail_times: usize,
}

impl ConfigProvider for FlakyWatchProvider {
    fn load(
        &self,
        _config: &mut R2eConfig,
        _ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError> {
        Ok(())
    }

    fn watch(
        self: Arc<Self>,
        _ctx: ConfigWatchContext,
        sink: ConfigUpdateSink,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConfigError>> + Send + 'static>> {
        Box::pin(async move {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt < self.fail_times {
                return Err(ConfigError::NotFound(self.key.to_string()));
            }
            sink.set(self.key, self.value.to_string());
            Ok(())
        })
    }
}

/// A watch that ERRORS is restarted — otherwise a single transient failure
/// disables runtime config updates for the life of the process (serve hooks
/// run once, and `r2e dev` skips them from the second cycle on).
#[tokio::test]
async fn failed_watch_is_restarted_until_it_succeeds() {
    let registry = LiveConfigRegistry::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn ConfigProvider> = Arc::new(FlakyWatchProvider {
        key: "db.url",
        value: "postgres://recovered",
        attempts: attempts.clone(),
        fail_times: 2,
    });

    let ctx = ConfigWatchContext::new("test", CancelToken::new());
    r2e_core::config::supervise_config_watch_with_backoff(
        provider,
        ctx,
        ConfigUpdateSink::new(registry.clone()),
        Duration::from_millis(1),
        Duration::from_millis(5),
    )
    .await;

    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(
        registry.get::<String>("db.url").unwrap(),
        "postgres://recovered"
    );
}

/// `Ok(())` is a deliberate end: a one-shot provider must NOT be looped.
#[tokio::test]
async fn watch_returning_ok_is_not_restarted() {
    let registry = LiveConfigRegistry::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn ConfigProvider> = Arc::new(FlakyWatchProvider {
        key: "db.url",
        value: "postgres://once",
        attempts: attempts.clone(),
        fail_times: 0,
    });

    let ctx = ConfigWatchContext::new("test", CancelToken::new());
    r2e_core::config::supervise_config_watch_with_backoff(
        provider,
        ctx,
        ConfigUpdateSink::new(registry),
        Duration::from_millis(1),
        Duration::from_millis(5),
    )
    .await;

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

/// Shutdown wins over the retry backoff — a permanently failing watch must not
/// keep a graceful drain waiting.
#[tokio::test]
async fn cancelled_shutdown_stops_the_retry_loop() {
    let registry = LiveConfigRegistry::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn ConfigProvider> = Arc::new(FlakyWatchProvider {
        key: "db.url",
        value: "never",
        attempts: attempts.clone(),
        fail_times: usize::MAX,
    });

    let token = CancelToken::new();
    let ctx = ConfigWatchContext::new("test", token.clone());
    let task = r2e_core::rt::spawn(r2e_core::config::supervise_config_watch_with_backoff(
        provider,
        ctx,
        ConfigUpdateSink::new(registry),
        Duration::from_millis(50),
        Duration::from_millis(50),
    ));

    tokio::time::sleep(Duration::from_millis(20)).await;
    token.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("supervision must stop as soon as the shutdown token fires")
        .unwrap();
    assert!(attempts.load(Ordering::SeqCst) >= 1);
}

/// A watch that never resolves: the provider is wedged (dead connection, a
/// `watch` that ignores its own token) rather than failing.
struct WedgedWatchProvider {
    entered: Arc<AtomicUsize>,
}

impl ConfigProvider for WedgedWatchProvider {
    fn load(
        &self,
        _config: &mut R2eConfig,
        _ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError> {
        Ok(())
    }

    fn watch(
        self: Arc<Self>,
        _ctx: ConfigWatchContext,
        _sink: ConfigUpdateSink,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConfigError>> + Send + 'static>> {
        Box::pin(async move {
            self.entered.fetch_add(1, Ordering::SeqCst);
            std::future::pending().await
        })
    }
}

/// Shutdown wins over the **in-flight watch**, not just over the retry sleep.
///
/// The supervisor awaits the provider's future; a provider that never resolves
/// (and never checks the token it was handed) would otherwise pin the
/// supervision task open for the whole drain. Racing the watch itself against
/// the token bounds that to "returns immediately".
#[tokio::test]
async fn cancelled_shutdown_aborts_an_in_flight_watch() {
    let registry = LiveConfigRegistry::new();
    let entered = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn ConfigProvider> = Arc::new(WedgedWatchProvider {
        entered: entered.clone(),
    });

    let token = CancelToken::new();
    let ctx = ConfigWatchContext::new("test", token.clone());
    let task = r2e_core::rt::spawn(r2e_core::config::supervise_config_watch_with_backoff(
        provider,
        ctx,
        ConfigUpdateSink::new(registry),
        // Long enough that a pass through the backoff sleep would blow the
        // timeout below: only cancelling the watch future itself can pass.
        Duration::from_secs(30),
        Duration::from_secs(30),
    ));

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        entered.load(Ordering::SeqCst),
        1,
        "the watch must have been entered before cancellation"
    );

    token.cancel();
    tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .expect("a never-resolving watch must not outlive the shutdown token")
        .unwrap();
}

/// A token already cancelled on entry must not touch the provider at all —
/// the `biased` select checks cancellation before polling the watch.
#[tokio::test]
async fn watch_is_not_started_when_shutdown_already_fired() {
    let registry = LiveConfigRegistry::new();
    let entered = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn ConfigProvider> = Arc::new(WedgedWatchProvider {
        entered: entered.clone(),
    });

    let token = CancelToken::new();
    token.cancel();
    let ctx = ConfigWatchContext::new("test", token);

    tokio::time::timeout(
        Duration::from_millis(500),
        r2e_core::config::supervise_config_watch_with_backoff(
            provider,
            ctx,
            ConfigUpdateSink::new(registry),
            Duration::from_secs(30),
            Duration::from_secs(30),
        ),
    )
    .await
    .expect("an already-cancelled token must return immediately");

    assert_eq!(entered.load(Ordering::SeqCst), 0);
}
