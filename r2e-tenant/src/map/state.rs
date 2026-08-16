//! The map and what it holds: [`Tenanted<T>`] and its shared [`Inner`] —
//! the slots, the negative cache, the wiring and the counters.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use r2e_core::plugin::GraphHandle;
use tokio::sync::{Notify, OnceCell};

use crate::source::TenantSource;
use crate::TenantId;

use super::TenantedSettings;

#[allow(unused_imports)]
use super::dispose::Pending;
#[allow(unused_imports)]
use crate::source::TenantContext;

/// Every tenant's copy of `T`, created on demand.
pub struct Tenanted<T> {
    pub(super) inner: Arc<Inner<T>>,
}

impl<T> Clone for Tenanted<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

pub(super) struct Inner<T> {
    pub(super) slots: DashMap<TenantId, Arc<Slot<T>>>,
    pub(super) negative: DashMap<TenantId, u64>,
    pub(super) wiring: Wiring<T>,
    /// Time base for the millisecond stamps on slots and negative entries.
    pub(super) started: Instant,
    /// Bumped by every removal, so a creation that started before one can tell.
    ///
    /// See [`Tenanted::bump_epoch`].
    pub(super) epoch: AtomicU64,
    pub(super) trimming: AtomicBool,
    /// Latched by [`Tenanted::drain`]: shutdown started, nothing new is served.
    pub(super) draining: AtomicBool,
    /// Work that keeps a value alive outside the map, counted so `drain` can
    /// wait for it. See [`Pending`].
    pub(super) in_flight: AtomicUsize,
    /// Woken every time [`Inner::in_flight`] falls back to zero.
    pub(super) settled: Notify,
    pub(super) counters: Counters,
}

pub(super) struct Slot<T> {
    pub(super) cell: OnceCell<T>,
    pub(super) last_used: AtomicU64,
    /// One-shot gate: whoever wins it calls `dispose`, everyone else skips.
    pub(super) disposed: AtomicBool,
    /// The removal epoch this slot's *current* initialization started at.
    ///
    /// On the slot, not on the resolver, so that every participant sharing this
    /// cell — the task running the initializer and every waiter parked on it —
    /// classifies the one value they share identically. Per-participant captures
    /// let two of them disagree, and the disagreement is not benign: one would
    /// spawn disposal while the other put the same slot back, caching a disposed
    /// resource.
    pub(super) epoch: AtomicU64,
}

impl<T> Slot<T> {
    pub(super) fn new(now: u64, epoch: u64) -> Self {
        Self {
            cell: OnceCell::new(),
            last_used: AtomicU64::new(now),
            disposed: AtomicBool::new(false),
            epoch: AtomicU64::new(epoch),
        }
    }

    /// Whether the resource is built (as opposed to a creation in flight).
    pub(super) fn is_ready(&self) -> bool {
        self.cell.initialized()
    }

    /// Whether the one-shot disposal gate has been taken.
    ///
    /// `true` means the value is dead or being closed right now: nothing may
    /// cache it again.
    pub(super) fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }

    /// The epoch this initialization started at.
    pub(super) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
}

pub(super) struct Wiring<T> {
    pub(super) source: Arc<dyn TenantSource<T>>,
    /// The resolved bean graph, filled by the framework after `build_state()`
    /// (or by the embedder). Backs [`TenantContext::bean`] and the cascade,
    /// both of which only run at request time — after the fill.
    ///
    /// The handle is **weak** (this map lives *in* the graph it points at, so
    /// a strong one would be a self-sustaining cycle); the router owns the
    /// graph, so it is alive for every request that can reach us and gone only
    /// after the app is dropped.
    pub(super) graph: GraphHandle,
    pub(super) settings: TenantedSettings,
    /// The app-scoped default, when `fallback_to_default()` was asked for.
    pub(super) fallback: Option<T>,
}

#[derive(Default)]
pub(super) struct Counters {
    pub(super) hits: AtomicU64,
    pub(super) created: AtomicU64,
    pub(super) create_failures: AtomicU64,
    pub(super) timeouts: AtomicU64,
    pub(super) unknown: AtomicU64,
    pub(super) fallbacks: AtomicU64,
    pub(super) disposed: AtomicU64,
    pub(super) evicted_idle: AtomicU64,
    pub(super) evicted_lru: AtomicU64,
}

impl<T> std::fmt::Debug for Tenanted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tenanted")
            .field("resource", &std::any::type_name::<T>())
            .field("slots", &self.inner.slots.len())
            .finish()
    }
}
