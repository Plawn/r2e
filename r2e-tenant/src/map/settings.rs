//! [`TenantedSettings`] — the per-resource knobs a map runs on.

use std::time::Duration;

use crate::config::{TenancyConfig, DEFAULT_MAX_ACTIVE, DEFAULT_MAX_NEGATIVE};
use crate::error::TenantStatuses;

/// Per-resource knobs: the `tenancy.*` defaults after the
/// [`PerTenant`](crate::PerTenant) builder overrides have been applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantedSettings {
    /// Cap on live per-tenant resources; the excess is evicted least-recent-first.
    ///
    /// A **soft** cap, enforced by a background trim rather than by admission
    /// control: a cold burst can briefly exceed it, and the trim runs until the
    /// map is back under the cap. `0` is rejected at wiring time.
    pub max_active: usize,
    /// Evict a resource unused for this long. `None` disables idle eviction.
    pub idle_ttl: Option<Duration>,
    /// Budget for one `create` call. `None` disables the timeout.
    pub create_timeout: Option<Duration>,
    /// How long an unknown tenant is remembered. `None` disables negative caching.
    pub negative_ttl: Option<Duration>,
    /// Cap on negative-cache entries.
    pub max_negative: usize,
    /// How tenancy failures map to HTTP statuses.
    pub statuses: TenantStatuses,
}

impl Default for TenantedSettings {
    fn default() -> Self {
        Self {
            max_active: DEFAULT_MAX_ACTIVE,
            idle_ttl: crate::config::TenancyConfig::default().idle_ttl(),
            create_timeout: crate::config::TenancyConfig::default().create_timeout(),
            negative_ttl: crate::config::TenancyConfig::default().negative_ttl(),
            max_negative: DEFAULT_MAX_NEGATIVE,
            statuses: TenantStatuses::default(),
        }
    }
}

impl TenantedSettings {
    /// The settings a `tenancy.*` section asks for, before per-resource
    /// overrides.
    #[must_use]
    pub fn from_config(config: &TenancyConfig) -> Self {
        Self {
            max_active: config.max_active(),
            idle_ttl: config.idle_ttl(),
            create_timeout: config.create_timeout(),
            negative_ttl: config.negative_ttl(),
            max_negative: config.max_negative(),
            statuses: config.statuses(),
        }
    }

    /// How often the background sweep runs: a quarter of `idle-ttl`, clamped to
    /// `[1s, 60s]` — often enough that eviction is timely, rarely enough that an
    /// idle app stays idle.
    #[must_use]
    pub fn sweep_interval(&self) -> Duration {
        let base = self.idle_ttl.unwrap_or(Duration::from_secs(120)) / 4;
        base.clamp(Duration::from_secs(1), Duration::from_secs(60))
    }
}
