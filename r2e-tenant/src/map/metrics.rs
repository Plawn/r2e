//! What a map reports: [`TenantedMetrics`], [`TenantStats`] and
//! [`SweepReport`].

use std::time::Duration;

use crate::TenantId;

#[allow(unused_imports)]
use super::Tenanted;

/// A point-in-time view of one [`Tenanted<T>`] map.
///
/// `Serialize` so an admin endpoint is `Json(map.metrics())`, not a hand-built
/// object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct TenantedMetrics {
    /// Tenants with a live resource right now.
    pub active: usize,
    /// Unknown tenants currently remembered.
    pub negative: usize,
    /// Resources served from cache.
    pub hits: u64,
    /// Resources created.
    pub created: u64,
    /// `create` calls that returned an error.
    pub create_failures: u64,
    /// `create` calls that hit `create-timeout`.
    pub timeouts: u64,
    /// `create` calls that reported an unknown tenant.
    pub unknown: u64,
    /// Requests served with the app-scoped fallback bean.
    pub fallbacks: u64,
    /// Resources handed to `dispose`.
    pub disposed: u64,
    /// Resources evicted for being idle.
    pub evicted_idle: u64,
    /// Resources evicted to stay under `max-active`.
    pub evicted_lru: u64,
}

/// Per-tenant state, as reported by [`Tenanted::stats`].
///
/// `Serialize` so an admin endpoint is `Json(map.stats())`; `idle` is emitted as
/// whole milliseconds.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TenantStats {
    /// The tenant.
    pub tenant: TenantId,
    /// Whether its resource is built (`false` = creation in flight).
    pub ready: bool,
    /// Time since its last use.
    #[serde(rename = "idle_ms", serialize_with = "serialize_millis")]
    pub idle: Duration,
}

/// `Duration` has no stable JSON shape; milliseconds is what the rest of the
/// tenancy surface talks in.
fn serialize_millis<S: serde::Serializer>(idle: &Duration, ser: S) -> Result<S::Ok, S::Error> {
    ser.serialize_u64(idle.as_millis() as u64)
}

/// What one [`Tenanted::sweep`] removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Resources evicted for being idle.
    pub idle_evicted: usize,
    /// Resources evicted to stay under `max-active`.
    pub lru_evicted: usize,
    /// Expired negative-cache entries dropped.
    pub negative_purged: usize,
}

impl SweepReport {
    /// Whether the sweep removed anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}
