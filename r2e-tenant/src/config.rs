//! The `tenancy.*` config section.
//!
//! One section for the whole layer: the resolution policy (what happens when a
//! request carries no tenant, and which statuses failures map to) plus the
//! **defaults** for every [`Tenanted<T>`](crate::Tenanted) map. Per-resource
//! overrides live on the [`PerTenant`](crate::PerTenant) builder, which always
//! wins over the file:
//!
//! ```text
//! builder setting  >  tenancy.* (file)  >  built-in default
//! ```
//!
//! ```yaml
//! tenancy:
//!   enabled: true
//!   on-missing: reject        # reject | allow
//!   missing-status: 400
//!   unknown-status: 404
//!   unavailable-status: 503
//!   max-active: 500
//!   idle-ttl: 15m
//!   create-timeout: 10s
//!   negative-ttl: 5s
//!   max-negative: 1024
//! ```

use std::time::Duration;

use r2e_core::http::StatusCode;
use r2e_core::prelude::ConfigProperties;

use crate::error::TenantStatuses;

/// Default `max-active`: per-tenant resources kept alive per `Tenanted<T>`.
pub const DEFAULT_MAX_ACTIVE: usize = 500;
/// Default `idle-ttl`: how long an unused per-tenant resource is kept.
pub const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(15 * 60);
/// Default `create-timeout`: budget for one `TenantSource::create` call.
pub const DEFAULT_CREATE_TIMEOUT: Duration = Duration::from_secs(10);
/// Default `negative-ttl`: how long an unknown tenant is remembered.
pub const DEFAULT_NEGATIVE_TTL: Duration = Duration::from_secs(5);
/// Default `max-negative`: cap on remembered unknown tenants.
pub const DEFAULT_MAX_NEGATIVE: usize = 1024;

/// What to do with a request that carries no tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingTenantPolicy {
    /// Fail the request (`missing-status`, 400 by default) — **including**
    /// `Option<Tenant<T>>` / `Option<TenantId>` extractors. The default: in a
    /// multi-tenant deployment a request without a tenant is malformed, and
    /// failing closed keeps a handler from silently serving cross-tenant data.
    #[default]
    Reject,
    /// Tolerate it: `Option` extractors yield `None`. Required extractors
    /// (`Tenant<T>`, `TenantId`) still fail with `missing-status` — there is no
    /// tenant to serve them. Use this for apps that mix tenant-scoped and
    /// tenant-less routes.
    Allow,
}

impl MissingTenantPolicy {
    /// The `on-missing` value that selects this policy.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Allow => "allow",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "reject" => Some(Self::Reject),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }
}

/// Typed view of the `tenancy.*` section.
///
/// Every field is optional: absent means "use the built-in default", which is
/// what lets a per-resource [`PerTenant`](crate::PerTenant) builder setting take
/// precedence over the file without the file's defaults masking it.
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct TenancyConfig {
    /// `tenancy.enabled` (default `true`). `false` installs inert shells: the
    /// app boots, `Option` extractors yield `None`, required ones fail with
    /// `missing-status`.
    pub enabled: Option<bool>,
    /// `reject` (default) or `allow` — see [`MissingTenantPolicy`].
    #[config(key = "on-missing")]
    pub on_missing: Option<String>,
    /// Status for "no tenant in request" (default 400).
    #[config(key = "missing-status")]
    pub missing_status: Option<u64>,
    /// Status for "tenant not provisioned" (default 404).
    #[config(key = "unknown-status")]
    pub unknown_status: Option<u64>,
    /// Status for "tenant resource could not be built" (default 503).
    #[config(key = "unavailable-status")]
    pub unavailable_status: Option<u64>,
    /// Default per-`Tenanted<T>` cap on live resources (default 500).
    #[config(key = "max-active")]
    pub max_active: Option<u64>,
    /// Default idle eviction delay (default 15m). `0` disables idle eviction.
    #[config(key = "idle-ttl")]
    pub idle_ttl: Option<Duration>,
    /// Default per-create timeout (default 10s). `0` disables the timeout.
    #[config(key = "create-timeout")]
    pub create_timeout: Option<Duration>,
    /// Default negative-cache TTL (default 5s). `0` disables negative caching.
    #[config(key = "negative-ttl")]
    pub negative_ttl: Option<Duration>,
    /// Default cap on negative-cache entries (default 1024).
    #[config(key = "max-negative")]
    pub max_negative: Option<u64>,
}

impl TenancyConfig {
    /// Whether the tenancy layer is enabled (default `true`).
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// The missing-tenant policy (default [`MissingTenantPolicy::Reject`]).
    ///
    /// # Panics
    ///
    /// Panics at boot on an unknown value — a typo in `on-missing` would
    /// otherwise silently pick the wrong side of a fail-closed switch.
    #[must_use]
    pub fn policy(&self) -> MissingTenantPolicy {
        match &self.on_missing {
            None => MissingTenantPolicy::default(),
            Some(raw) => MissingTenantPolicy::parse(raw).unwrap_or_else(|| {
                panic!(
                    "invalid `tenancy.on-missing` value {raw:?}: expected \"reject\" or \"allow\""
                )
            }),
        }
    }

    /// The configured HTTP statuses.
    ///
    /// # Panics
    ///
    /// Panics at boot when a status is not a valid HTTP status code.
    #[must_use]
    pub fn statuses(&self) -> TenantStatuses {
        let defaults = TenantStatuses::default();
        TenantStatuses {
            missing: status(self.missing_status, "missing-status", defaults.missing),
            unknown: status(self.unknown_status, "unknown-status", defaults.unknown),
            unavailable: status(
                self.unavailable_status,
                "unavailable-status",
                defaults.unavailable,
            ),
        }
    }

    /// `max-active`, or the built-in default.
    ///
    /// # Panics
    ///
    /// Panics at boot on `max-active: 0`. A cap of zero would create every
    /// tenant's resource and evict it straight away; it is a typo, not a way to
    /// disable per-tenant resources (that is `tenancy.enabled: false`).
    #[must_use]
    pub fn max_active(&self) -> usize {
        self.max_active.map_or(DEFAULT_MAX_ACTIVE, |v| {
            assert!(
                v > 0,
                "invalid `tenancy.max-active` value 0: expected at least 1 \
                 (use `tenancy.enabled: false` to turn tenancy off)"
            );
            usize::try_from(v).unwrap_or(usize::MAX)
        })
    }

    /// `idle-ttl`, or the built-in default. `None` = idle eviction disabled.
    #[must_use]
    pub fn idle_ttl(&self) -> Option<Duration> {
        zeroable(self.idle_ttl, DEFAULT_IDLE_TTL)
    }

    /// `create-timeout`, or the built-in default. `None` = no timeout.
    #[must_use]
    pub fn create_timeout(&self) -> Option<Duration> {
        zeroable(self.create_timeout, DEFAULT_CREATE_TIMEOUT)
    }

    /// `negative-ttl`, or the built-in default. `None` = no negative caching.
    #[must_use]
    pub fn negative_ttl(&self) -> Option<Duration> {
        zeroable(self.negative_ttl, DEFAULT_NEGATIVE_TTL)
    }

    /// `max-negative`, or the built-in default.
    #[must_use]
    pub fn max_negative(&self) -> usize {
        self.max_negative.map_or(DEFAULT_MAX_NEGATIVE, |v| {
            usize::try_from(v).unwrap_or(usize::MAX)
        })
    }
}

fn zeroable(configured: Option<Duration>, default: Duration) -> Option<Duration> {
    match configured {
        None => Some(default),
        Some(d) if d.is_zero() => None,
        Some(d) => Some(d),
    }
}

fn status(configured: Option<u64>, key: &str, default: StatusCode) -> StatusCode {
    match configured {
        None => default,
        Some(raw) => u16::try_from(raw)
            .ok()
            .and_then(|v| StatusCode::from_u16(v).ok())
            .unwrap_or_else(|| {
                panic!("invalid `tenancy.{key}` value {raw}: expected an HTTP status code")
            }),
    }
}
