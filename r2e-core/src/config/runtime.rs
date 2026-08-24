use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use arc_swap::ArcSwapOption;

use crate::rt::sync::watch;
use crate::rt::CancelToken;

use super::{ConfigError, ConfigValue, FromConfigValue, R2eConfig};

/// Context passed to boot-time config providers.
#[derive(Clone, Copy, Debug)]
pub struct ConfigProviderContext<'a> {
    pub profile: &'a str,
}

/// Context passed to runtime provider watchers.
#[derive(Clone, Debug)]
pub struct ConfigWatchContext {
    profile: String,
    shutdown: CancelToken,
}

impl ConfigWatchContext {
    #[must_use]
    pub fn new(profile: impl Into<String>, shutdown: crate::rt::CancelToken) -> Self {
        Self {
            profile: profile.into(),
            shutdown: shutdown.into(),
        }
    }

    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    #[must_use]
    pub fn shutdown_token(&self) -> crate::rt::CancelToken {
        self.shutdown.clone().into()
    }
}

/// External source of configuration values.
pub trait ConfigProvider: Send + Sync + 'static {
    /// Load provider values during `load_config`, before typed config is built.
    fn load(
        &self,
        config: &mut R2eConfig,
        ctx: ConfigProviderContext<'_>,
    ) -> Result<(), ConfigError>;

    /// Watch for runtime updates.
    ///
    /// Providers that do not support runtime watching can keep the default
    /// implementation.
    ///
    /// The return value is a **contract**, because the watch task is
    /// supervised (see [`supervise_config_watch`]):
    ///
    /// - `Ok(())` — done watching on purpose (the default impl, or a one-shot
    ///   provider that pushes once). The supervisor stops; the provider is not
    ///   called again.
    /// - `Err(_)` — the watch broke (connection dropped, lease expired, …).
    ///   The supervisor logs it and calls `watch` again after a capped
    ///   exponential backoff, so a transient failure does not silently disable
    ///   runtime config updates for the rest of the process' life.
    ///
    /// Long-lived watchers should therefore run until
    /// [`ConfigWatchContext::shutdown_token`] fires and return `Ok(())` then —
    /// returning `Ok(())` early means "never call me again".
    fn watch(
        self: Arc<Self>,
        _ctx: ConfigWatchContext,
        _sink: ConfigUpdateSink,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConfigError>> + Send + 'static>> {
        Box::pin(async { Ok(()) })
    }
}

/// First retry delay after a failed [`ConfigProvider::watch`].
const WATCH_BACKOFF_INITIAL: std::time::Duration = std::time::Duration::from_secs(1);
/// Cap for the exponential retry delay.
const WATCH_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(30);
/// A watch attempt that ran at least this long is considered healthy: the next
/// failure restarts from [`WATCH_BACKOFF_INITIAL`] instead of the grown delay.
const WATCH_HEALTHY_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// Run one provider's [`watch`](ConfigProvider::watch) under supervision until
/// shutdown.
///
/// A watch task used to be started exactly once and never restarted: a
/// provider whose watch returned an error stopped feeding the
/// [`LiveConfigRegistry`] for the whole life of the process (and under
/// `r2e dev`, serve hooks are skipped from the second hot-patch cycle on, so
/// nothing would have restarted it there either). This supervisor restarts a
/// **failed** watch with a capped exponential backoff and treats `Ok(())` as a
/// deliberate end — see [`ConfigProvider::watch`] for the contract.
///
/// Cancellation is honoured at every point of the cycle: before an attempt,
/// **during** the in-flight `watch` future, and during the retry sleep. A
/// provider that never resolves therefore cannot hold up a graceful drain.
pub async fn supervise_config_watch(
    provider: Arc<dyn ConfigProvider>,
    ctx: ConfigWatchContext,
    sink: ConfigUpdateSink,
) {
    supervise_config_watch_with_backoff(
        provider,
        ctx,
        sink,
        WATCH_BACKOFF_INITIAL,
        WATCH_BACKOFF_MAX,
    )
    .await;
}

/// [`supervise_config_watch`] with explicit backoff bounds — tests only.
#[doc(hidden)]
pub async fn supervise_config_watch_with_backoff(
    provider: Arc<dyn ConfigProvider>,
    ctx: ConfigWatchContext,
    sink: ConfigUpdateSink,
    initial_backoff: std::time::Duration,
    max_backoff: std::time::Duration,
) {
    let shutdown = ctx.shutdown_token();
    let mut delay = initial_backoff;
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        let started = std::time::Instant::now();
        // The watch future itself races the shutdown token, not just the
        // retry sleep: a provider that never resolves (wedged connection,
        // a `watch` that ignores its own shutdown token) would otherwise
        // hold the graceful drain open for as long as it stays stuck.
        // `biased` checks cancellation before polling the watch, so a token
        // already cancelled on entry returns without touching the provider.
        let outcome = crate::rt::select! {
            biased;
            () = shutdown.cancelled() => return,
            outcome = provider.clone().watch(ctx.clone(), sink.clone()) => outcome,
        };
        match outcome {
            Ok(()) => return,
            Err(error) => {
                if shutdown.is_cancelled() {
                    return;
                }
                if started.elapsed() >= WATCH_HEALTHY_AFTER {
                    delay = initial_backoff;
                }
                tracing::warn!(
                    error = %error,
                    retry_in_ms = delay.as_millis() as u64,
                    "config provider watch failed — restarting"
                );
                crate::rt::select! {
                    () = shutdown.cancelled() => return,
                    () = crate::rt::sleep(delay) => {}
                }
                delay = std::cmp::min(delay.saturating_mul(2), max_backoff);
            }
        }
    }
}

/// A versioned snapshot of one live-config value.
#[derive(Debug, Clone, Default)]
pub struct LiveConfigSnapshot {
    value: Option<ConfigValue>,
    version: u64,
    updated_at: Option<SystemTime>,
    last_error: Option<String>,
}

impl LiveConfigSnapshot {
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub fn updated_at(&self) -> Option<SystemTime> {
        self.updated_at
    }

    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// A previously published value is still being served while the latest
    /// refresh failed. Purely derived: an error with no value behind it is a
    /// plain failure, not staleness.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.last_error.is_some() && self.value.is_some()
    }

    pub fn get<T: FromConfigValue>(&self, key: &str) -> Result<T, ConfigError> {
        let value = self
            .value
            .as_ref()
            .ok_or_else(|| ConfigError::NotFound(key.to_string()))?;
        T::from_config_value(value, key)
    }
}

#[derive(Clone)]
struct LiveSlot {
    sender: watch::Sender<LiveConfigSnapshot>,
}

/// The boot config a registry seeds its lazy slots from.
///
/// Behind an `RwLock` because a dev-reload cycle **swaps** it (see
/// [`LiveConfigRegistry::reseed`]) — the registry instance is stable for the
/// whole process, only its boot snapshot moves.
struct BootSnapshot {
    /// Boot config snapshot, kept for lazy slot seeding. `R2eConfig` is
    /// `Arc`-backed, so this is a pointer clone, not a map copy.
    config: R2eConfig,
    /// Timestamp stamped on lazily seeded boot values, so `updated_at`
    /// reports boot time rather than first-access time.
    at: SystemTime,
}

struct LiveConfigRegistryInner {
    boot: RwLock<BootSnapshot>,
    slots: RwLock<HashMap<String, LiveSlot>>,
    pinned: RwLock<HashSet<String>>,
    /// Whether at least one [`ConfigProvider`] is registered on the app that
    /// owns this registry — i.e. whether *anything* is expected to write values
    /// this registry did not get from the boot config. Set by `load_config`;
    /// `false` for a hand-built registry, which is the correct default (nobody
    /// registered a writer, so nobody will). Only ever moves `false → true`.
    ///
    /// Read exclusively by the dead-key diagnostic — see
    /// [`LiveConfigRegistry::is_dead_key`].
    has_providers: AtomicBool,
}

impl Default for LiveConfigRegistryInner {
    fn default() -> Self {
        Self {
            boot: RwLock::new(BootSnapshot {
                config: R2eConfig::empty(),
                at: SystemTime::now(),
            }),
            slots: RwLock::default(),
            pinned: RwLock::default(),
            has_providers: AtomicBool::new(false),
        }
    }
}

/// Registry of runtime-updatable ("live") config values.
///
/// Provided automatically by `load_config`. Values pushed by config providers
/// through [`ConfigUpdateSink`] land here and become visible to every
/// [`LiveConfig`] handle for the same key. This carries **no** confidentiality
/// semantics — it is plain live/dynamic config (feature flags, timeouts, URLs,
/// credentials alike), unrelated to the boot-time `${...}` secret placeholders
/// resolved by [`SecretResolver`](super::SecretResolver).
#[derive(Clone, Default)]
pub struct LiveConfigRegistry {
    inner: Arc<LiveConfigRegistryInner>,
}

impl LiveConfigRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry over a boot config snapshot.
    ///
    /// Slots are **lazy**: no watch channel is created here. A slot is
    /// materialized on first access (handle creation, `get`, `set`, …) and
    /// seeded from `config` if the key was present at boot.
    #[must_use]
    pub fn from_config(config: &R2eConfig, pinned: HashSet<String>) -> Self {
        Self {
            inner: Arc::new(LiveConfigRegistryInner {
                boot: RwLock::new(BootSnapshot {
                    config: config.clone(),
                    at: SystemTime::now(),
                }),
                slots: RwLock::default(),
                pinned: RwLock::new(pinned),
                has_providers: AtomicBool::new(false),
            }),
        }
    }

    /// Record that the owning app registered at least one [`ConfigProvider`].
    ///
    /// Called by `load_config`. Silences the dead-key warning: with a provider
    /// in play, a key that is absent at boot may legitimately be filled in later
    /// by a `watch` task, so "absent" is not evidence of a typo.
    pub(crate) fn mark_has_providers(&self) {
        self.inner.has_providers.store(true, Ordering::Relaxed);
    }

    /// Whether `key` looks like a **dead** live key: nothing can ever give it a
    /// value.
    ///
    /// True when the registry has no registered [`ConfigProvider`] *and* the key
    /// has neither a boot value nor a value already pushed at runtime. A
    /// `#[live_config("db.ulr")]` typo lands here — live keys are deliberately
    /// never presence-validated (an absent value is legal; the handle's `get()`
    /// returns `NotFound`), so without this check a misspelt key fails silently
    /// for the life of the process.
    ///
    /// Exposed (hidden) so the diagnostic's precondition is assertable from the
    /// integration tests without capturing log output. Does **not** materialize
    /// a slot.
    #[doc(hidden)]
    #[must_use]
    pub fn is_dead_key(&self, key: &str) -> bool {
        if self.inner.has_providers.load(Ordering::Relaxed) {
            return false;
        }
        if let Some(slot) = self.inner.slots.read().unwrap().get(key) {
            if slot.sender.borrow().value.is_some() {
                return false;
            }
        }
        self.inner.boot.read().unwrap().config.raw(key).is_none()
    }

    /// Re-seed this registry for a new dev-reload cycle, keeping its identity.
    ///
    /// Under `r2e dev` the registry is carried across hot-patch cycles (see
    /// `crate::runtime::dev`), because every `#[live_config]` handle ever created binds
    /// one slot of one registry *permanently* — a fresh registry per cycle
    /// would leave those handles reading a discarded instance. A cycle's
    /// `load_config` therefore re-seeds the surviving instance instead of
    /// building a new one:
    ///
    /// 1. The boot snapshot is swapped for the freshly loaded config, so slots
    ///    materialized from now on seed from the new values.
    /// 2. The pinned set is **replaced** (not merged): the env overlay keys and
    ///    the drained `override_config_value` keys are both recomputed from
    ///    scratch every cycle.
    /// 3. Already-materialized slots are pushed the new value **only when the
    ///    boot value actually changed** between the old and new snapshot. This
    ///    is the rule that keeps a runtime provider push alive: the single
    ///    surviving `ConfigProvider::watch` task writes into this same
    ///    instance, and an unconditional re-push would revert its updates on
    ///    every unrelated patch. A key whose YAML value *did* change is
    ///    deliberately allowed to win over a runtime push — the edit is the
    ///    more recent human intent.
    ///
    /// Pinned keys are skipped entirely (same guarantee `set` gives), and a key
    /// dropped from the config pushes an absence, so `LiveConfig::get` starts
    /// reporting `NotFound` again.
    #[cfg(feature = "dev-reload")]
    pub(crate) fn reseed(&self, config: &R2eConfig, pinned: HashSet<String>) {
        let previous = {
            let mut boot = self.inner.boot.write().unwrap();
            std::mem::replace(
                &mut *boot,
                BootSnapshot {
                    config: config.clone(),
                    at: SystemTime::now(),
                },
            )
        };
        *self.inner.pinned.write().unwrap() = pinned;

        // Only slots that already exist need a push — the rest seed lazily from
        // the snapshot we just installed.
        let materialized: Vec<String> = self.inner.slots.read().unwrap().keys().cloned().collect();
        for key in materialized {
            if self.is_pinned(&key) {
                continue;
            }
            // "Did this key's value change?" answered with the project's
            // existing structural digest — `ConfigValue` holds an `f64` variant
            // and deliberately does not derive `PartialEq`.
            let keys = [key.as_str()];
            if config.config_fingerprint(&keys) == previous.config.config_fingerprint(&keys) {
                continue;
            }
            self.publish(&key, config.raw(&key).cloned());
        }
    }

    /// Create a typed handle bound to `key`'s slot.
    ///
    /// Warns when the key is [dead](Self::is_dead_key) — absent at boot with no
    /// provider that could ever fill it in. This is the only guard a typo'd
    /// `#[live_config("db.ulr")]` gets: live keys are never presence-validated,
    /// so the misspelt handle would otherwise return `NotFound` forever without
    /// a word. It stays a warning, not an error, because a key filled in later
    /// by application code (`LiveConfigRegistry::set`) is a legitimate pattern.
    #[must_use]
    pub fn live_config<T: FromConfigValue + Clone + Send + Sync + 'static>(
        &self,
        key: impl Into<String>,
    ) -> LiveConfig<T> {
        let key: Arc<str> = Arc::from(key.into());
        if self.is_dead_key(&key) {
            tracing::warn!(
                config.key = %key,
                "live config key '{key}' has no value and no config provider is registered: \
                 it will stay unset unless something calls `LiveConfigRegistry::set`. \
                 Check the key for a typo, or register a `ConfigProvider` that supplies it."
            );
        }
        LiveConfig {
            slot: self.slot(&key),
            key,
            cache: Arc::new(ArcSwapOption::empty()),
        }
    }

    pub fn get<T: FromConfigValue>(&self, key: &str) -> Result<T, ConfigError> {
        self.slot(key).sender.borrow().get::<T>(key)
    }

    #[must_use]
    pub fn snapshot(&self, key: &str) -> LiveConfigSnapshot {
        self.slot(key).sender.borrow().clone()
    }

    pub fn set(&self, key: impl Into<String>, value: impl Into<ConfigValue>) -> bool {
        let key = key.into();
        if self.is_pinned(&key) {
            return false;
        }
        self.publish(&key, Some(value.into()));
        true
    }

    /// Pin `key` **and** publish `value`, bypassing the pin check.
    ///
    /// The override primitive behind
    /// [`AppBuilder::override_config_value`](crate::AppBuilder::override_config_value)
    /// when it runs *after* `load_config`: the override must land in the live
    /// slot (so `#[live_config]` handles read it) **and** be protected from
    /// later provider writes. Pinning first and then calling
    /// [`set`](Self::set) cannot express that — `set` would refuse its own
    /// write.
    pub(crate) fn pin_set(&self, key: impl Into<String>, value: impl Into<ConfigValue>) {
        let key = key.into();
        // Same lock order as `set` (pinned → slots); released before touching
        // the slot map so the two are never held together.
        self.inner.pinned.write().unwrap().insert(key.clone());
        self.publish(&key, Some(value.into()));
    }

    pub fn set_error(&self, key: impl Into<String>, error: impl ToString) {
        let key = key.into();
        let slot = self.slot(&key);
        let mut snapshot = slot.sender.borrow().clone();
        snapshot.last_error = Some(error.to_string());
        let _ = slot.sender.send_replace(snapshot);
    }

    /// Publish `value` at a bumped version, bypassing the pin check.
    ///
    /// `None` publishes an *absence*, so `LiveConfig::get` starts reporting
    /// `NotFound` again — what a key dropped from the config between two
    /// dev-reload cycles needs.
    fn publish(&self, key: &str, value: Option<ConfigValue>) {
        let slot = self.slot(key);
        let mut snapshot = slot.sender.borrow().clone();
        snapshot.value = value;
        snapshot.version = snapshot.version.saturating_add(1);
        snapshot.updated_at = Some(SystemTime::now());
        snapshot.last_error = None;
        let _ = slot.sender.send_replace(snapshot);
    }

    fn is_pinned(&self, key: &str) -> bool {
        self.inner.pinned.read().unwrap().contains(key)
    }

    /// Resolve the slot for `key`, creating (and boot-seeding) it on first use.
    fn slot(&self, key: &str) -> LiveSlot {
        if let Some(slot) = self.inner.slots.read().unwrap().get(key).cloned() {
            return slot;
        }
        let mut slots = self.inner.slots.write().unwrap();
        slots
            .entry(key.to_string())
            .or_insert_with(|| {
                let (sender, _) = watch::channel(self.boot_snapshot(key));
                LiveSlot { sender }
            })
            .clone()
    }

    /// Initial snapshot for a freshly created slot: the boot value at version 1
    /// (exactly what the old eager seeding produced), or the empty default when
    /// the key was absent at boot.
    fn boot_snapshot(&self, key: &str) -> LiveConfigSnapshot {
        let boot = self.inner.boot.read().unwrap();
        match boot.config.raw(key) {
            Some(value) => LiveConfigSnapshot {
                value: Some(value.clone()),
                version: 1,
                updated_at: Some(boot.at),
                last_error: None,
            },
            None => LiveConfigSnapshot::default(),
        }
    }
}

/// Sink handed to provider watchers for runtime updates.
#[derive(Clone)]
pub struct ConfigUpdateSink {
    registry: LiveConfigRegistry,
}

impl ConfigUpdateSink {
    #[must_use]
    pub fn new(registry: LiveConfigRegistry) -> Self {
        Self { registry }
    }

    pub fn set(&self, key: impl Into<String>, value: impl Into<ConfigValue>) -> bool {
        self.registry.set(key, value)
    }

    pub fn set_error(&self, key: impl Into<String>, error: impl ToString) {
        self.registry.set_error(key, error);
    }

    #[must_use]
    pub fn registry(&self) -> LiveConfigRegistry {
        self.registry.clone()
    }
}

/// One cached typed conversion, tagged with the snapshot version it came from.
struct LiveCacheEntry<T> {
    version: u64,
    value: T,
}

/// Typed handle for one runtime-updatable config value.
///
/// The slot is resolved **once**, at handle creation — reads never touch the
/// registry's slot map. Each read compares the watch snapshot's version against
/// a shared typed cache and only runs [`FromConfigValue`] when the version
/// moved, so a per-request read costs an atomic load plus a `T` clone.
pub struct LiveConfig<T> {
    key: Arc<str>,
    slot: LiveSlot,
    /// Shared with every clone of this handle: one conversion per version per
    /// handle *family*, not per clone.
    cache: Arc<ArcSwapOption<LiveCacheEntry<T>>>,
}

impl<T> Clone for LiveConfig<T> {
    fn clone(&self) -> Self {
        Self {
            key: Arc::clone(&self.key),
            slot: self.slot.clone(),
            cache: Arc::clone(&self.cache),
        }
    }
}

impl<T: FromConfigValue + Clone + Send + Sync + 'static> LiveConfig<T> {
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Read the current value, converting only when the snapshot version moved.
    ///
    /// Version and value are published together by a single `send_replace`, so
    /// one `borrow()` reads a consistent pair: a cache entry tagged with the
    /// version we just observed is by construction the conversion of the value
    /// we just observed. A cached entry is therefore never served for a newer
    /// snapshot. Conversion failures are not cached — the handle keeps
    /// returning `Err` (and re-attempting) until a new value arrives.
    ///
    /// The version/value pair is taken out of the watch borrow before
    /// [`FromConfigValue`] runs: user conversion code is arbitrary and must not
    /// execute while the channel's read guard is held (it would block every
    /// publisher of this key). The extra `ConfigValue` clone that buys is on the
    /// miss path only — once per version, not per read.
    pub fn get(&self) -> Result<T, ConfigError> {
        let (version, raw) = {
            let snapshot = self.slot.sender.borrow();
            if let Some(entry) = self.cache.load().as_ref() {
                if entry.version == snapshot.version {
                    return Ok(entry.value.clone());
                }
            }
            (snapshot.version, snapshot.value.clone())
        };
        // A missing value caches nothing: the key stays `NotFound` until a
        // runtime `set` publishes one.
        let raw = raw.ok_or_else(|| ConfigError::NotFound(self.key.to_string()))?;
        let converted = T::from_config_value(&raw, &self.key)?;
        // Racing readers on the same version store equivalent entries — benign.
        self.cache.store(Some(Arc::new(LiveCacheEntry {
            version,
            value: converted.clone(),
        })));
        Ok(converted)
    }

    #[must_use]
    pub fn snapshot(&self) -> LiveConfigSnapshot {
        self.slot.sender.borrow().clone()
    }

    #[must_use]
    pub fn subscribe(&self) -> LiveConfigReceiver<T> {
        LiveConfigReceiver {
            key: Arc::clone(&self.key),
            receiver: self.slot.sender.subscribe(),
            _marker: PhantomData,
        }
    }
}

/// Watch receiver for one typed live-config value.
pub struct LiveConfigReceiver<T> {
    key: Arc<str>,
    receiver: watch::Receiver<LiveConfigSnapshot>,
    _marker: PhantomData<T>,
}

impl<T: FromConfigValue> LiveConfigReceiver<T> {
    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.receiver.changed().await
    }

    pub fn borrow_and_update(&mut self) -> Result<T, ConfigError> {
        self.receiver.borrow_and_update().get::<T>(&self.key)
    }

    #[must_use]
    pub fn snapshot(&self) -> LiveConfigSnapshot {
        self.receiver.borrow().clone()
    }

    /// Deliver the current value, then every subsequent one, until `shutdown`
    /// fires or the registry is dropped.
    ///
    /// The canonical body of a [`ServiceComponent`](crate::ServiceComponent)
    /// that owns a resource rebuilt from a live-config value (a pool, a client,
    /// a connection): the component supplies only *what to do with a value*,
    /// and the initial read, the change loop, the shutdown arm and the
    /// closed-channel exit all live here rather than being re-derived per
    /// resource.
    ///
    /// `on_value` receives the conversion `Result`, so a value that stops being
    /// convertible is reported rather than silently skipped.
    pub async fn drive<F, Fut>(mut self, shutdown: crate::rt::CancelToken, mut on_value: F)
    where
        F: FnMut(Result<T, ConfigError>) -> Fut,
        Fut: Future<Output = ()>,
    {
        on_value(self.borrow_and_update()).await;
        loop {
            crate::rt::select! {
                _ = shutdown.cancelled() => break,
                changed = self.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    on_value(self.borrow_and_update()).await;
                }
            }
        }
    }
}
