//! [`Tenanted<T>`] — one bean holding every tenant's copy of `T`.
//!
//! `Tenanted<T>` is an ordinary app-scoped bean (`Clone` = refcount bump) whose
//! job is to turn a [`TenantId`] into a `T`, creating it on first use, caching
//! it, and disposing of it when it is evicted. One `TypeId` per resource type,
//! so the state can carry `Tenanted<Pool<Postgres>>` and `Tenanted<ApiClient>`
//! side by side and the extractors can demand exactly the one they need.
//!
//! # Invariants
//!
//! These are the properties the implementation is built to keep; they are what
//! the tests in `tests/tenant/map.rs` pin down.
//!
//! - **Single flight.** N concurrent requests for a cold tenant produce exactly
//!   one `create` call. The map holds `Arc<Slot<T>>` values and the `Arc` is
//!   cloned out of the `DashMap` *before* any `.await` — holding a shard guard
//!   across an await would deadlock the map.
//! - **Failures are never cached.** An `Err` from `create` leaves nothing
//!   behind: the empty slot is removed (guarded by `Arc::ptr_eq` *and*
//!   "not ready", so neither a concurrent retry's slot nor a value that landed
//!   in this one meanwhile is ever stolen) and the next request tries again.
//!   That also means a flood of made-up tenant ids cannot accumulate slots.
//!   The removal also runs when the initializer **panics** or the future
//!   *running the initializer* is **cancelled** mid-`create` (a drop guard),
//!   so a hostile id that selects a panicking source path cannot accumulate
//!   empty slots either. A caller that never runs an initializer — a waiter
//!   parked behind someone else's creation — arms no guard, so its cancellation
//!   cannot detach the creation it was waiting for.
//!   Residual behaviour, inherited from `tokio::sync::OnceCell`: a failed
//!   initialization does not fail the waiters parked on that cell — they take
//!   turns running the initializer themselves, on the cell the failed attempt's
//!   cleanup just detached. Such a retry **reattaches** its slot when it
//!   succeeds (see below), so the value it produces is the map's. The negative
//!   cache is re-checked as the first step *inside* the initializer, so an
//!   unknown-tenant wave still costs one `create` call; an erroring wave can
//!   retry per waiter, which is deliberate (an error is not cached).
//! - **Unknown tenants are cached, briefly.** `Ok(None)` is remembered for
//!   `negative-ttl` (bounded by `max-negative` entries: an insert that goes over
//!   the bound purges expired entries and then drops arbitrary older ones, so
//!   the cache never *stays* over its bound) so a hot 404 does not hammer the
//!   tenant directory; any later success clears the entry.
//! - **Creation is bounded.** `create` runs under `create-timeout`; blowing it is
//!   a `504`, and every waiter parked on the slot is released.
//! - **Idle resources go away.** A background sweep (the [`ServiceComponent`]
//!   impl, started by the [`PerTenant`](crate::PerTenant) plugin) evicts
//!   resources unused for `idle-ttl`, trims the map to `max-active` by least
//!   recent use, and disposes what it removes. Shutdown drains everything.
//! - **`max-active` is a soft cap.** There is no admission control on creation:
//!   a cold burst of N tenants creates N resources and the *background trim*
//!   (kicked off by each completed creation, and re-run — clearing its flag and
//!   re-checking, so a completion that arrived mid-trim is not lost — until the
//!   map is back under the cap or nothing is left to evict) brings the map back
//!   down. Do not read `max_connections × max-active` as a hard capacity bound.
//! - **Removal only touches ready slots.** `evict`, `invalidate`, the sweeps and
//!   `drain` remove a tenant only when its cell is initialized; an in-flight
//!   creation is left mapped and completes into the still-mapped slot, so the
//!   value it produces is always the one the map owns and always reaches
//!   `dispose`. The one removal that can hit a *not ready* slot is the cleanup
//!   of a cancelled or panicking initializer — it removes the slot it was
//!   itself creating into. A waiter that inherits that cell and succeeds
//!   **reattaches** it (`Vacant` → put back; occupied by a *different* slot →
//!   the value is orphaned, disposed of in the background, and still handed to
//!   its caller, which the no-lease contract below already allows). Under the
//!   `draining` latch nothing is reattached: the value is disposed of and the
//!   caller gets the 503.
//! - **A public removal fences the creations that predate it.** `evict`,
//!   `invalidate`, the sweeps and `drain` bump a map-wide epoch *before* they
//!   take the key's shard lock, and every initialization stamps the epoch it
//!   started at **on its slot** — one reading shared by every participant on
//!   that cell, rather than a per-caller capture. A *detached* completion may
//!   only write back — reattach its
//!   slot, or remember the tenant as unknown — when the epoch is unchanged: a
//!   vacant key reads the same whether nobody ever mapped the tenant or an
//!   `invalidate` just emptied it, and resurrecting a pre-invalidation value (or
//!   the negative entry `invalidate` cleared) would break the documented
//!   immediacy of those calls. Both write-backs decide and write under the same
//!   `slots` shard guard, which is what orders them against the removal. A
//!   fenced value is orphaned: disposed of, and still returned to its own
//!   caller. The epoch is deliberately map-wide, so a removal can fence an
//!   unrelated tenant's detached creation — the cost is one rebuild. Creations
//!   that are still *mapped* are not fenced: removal never touches them, so they
//!   keep the slot the map owns; such a creation **overlaps** the removal and is
//!   deliberately left alone.
//!
//!   The one removal that does **not** bump is the cleanup of a cancelled or
//!   panicking initializer's empty slot, and that is deliberate: bumping there
//!   would fence off the very waiter that inherits the cell, and the legitimate
//!   self-heal above could never happen. So the epoch alone does not settle who
//!   owns a value — that is the gate rule below.
//! - **Disposal happens at most once per cached value, and a disposed value is
//!   never the map's.** Every slot carries a one-shot gate, so a concurrent
//!   `evict` + `drain` (or two sweeps) hand a value to
//!   [`TenantSource::dispose`] once and only once. The gate commits *before* the
//!   call, so a `dispose` that panics or is cancelled mid-await is **not**
//!   retried — a deliberate trade against ever double-disposing.
//!
//!   The same gate is what keeps a dying value out of the map. Two participants
//!   sharing a cell *can* classify its value differently — a competing empty
//!   slot appears under the key, one of them orphans against it, and then that
//!   slot vanishes (its initializer failed, and that cleanup does not bump the
//!   epoch), leaving the next participant looking at a vacant key at a matching
//!   epoch. What closes that window is where the gate is taken: **inline, under
//!   the key's shard guard, in the same critical section as the classification**
//!   — so either the orphan commits first and the restore reads `is_disposed()`
//!   under that same lock and refuses, or the restore lands first and the orphan
//!   finds its own slot back under the key (`ptr_eq` → kept, no gate, no
//!   disposal). Committing inside the spawned disposal task instead would leave
//!   the window open for the whole scheduling delay.
//!
//!   The rule is uniform, with no exceptions: **whoever unmaps or orphans a
//!   value commits its gate under the key's shard lock**, in the same critical
//!   section as the decision — `take_ready` inside its `remove_if` predicate,
//!   `take_slot` and `reattach` inside a bound `Entry` guard. There is no gate
//!   CAS anywhere else. That is what makes `evict().await` mean what it says: a
//!   participant arriving a moment later reads the committed gate and stands
//!   down instead of taking the closing over onto a detached task. Exactly one
//!   caller ever owes the `dispose` await: the one that won the CAS.
//! - **`drain` returns only once everything it is draining is closed.** Walking
//!   the map is not enough on its own: a live value can be *outside* it and
//!   still need closing — a resolve holding a slot that was detached under it
//!   (the cancelled-initializer case above), or a disposal somebody committed
//!   and handed to a detached task. Both mint a counted in-flight guard the
//!   instant they come into being — the disposal one *inside* the same
//!   shard-lock critical section that took the gate — and `drain` waits for that
//!   count to reach zero as well as for the map to come up empty. So `drain`
//!   also waits for a creation that is still in flight, rather than leaving it
//!   to close itself behind shutdown's back (bounded by `create-timeout` when
//!   one is configured).
//!
//!   What it does **not** wait for is traffic that arrives after the latch.
//!   `resolve` admits work through a double check — read the latch, *then*
//!   count, then read it again — so a post-shutdown request is rejected by the
//!   first read without ever touching the counter. Only the finite set already
//!   past that read when the latch went up is ever counted, which is what stops
//!   a sustained flood of 503s from holding the counter above zero and starving
//!   shutdown (the listener is still accepting while this hook runs, so that
//!   flood is an ordinary shape). Racing a manual `evict`/`invalidate` against
//!   `drain` is outside the invariant: the latch does not fence those.
//! - **What a handed-out value does *not* get is a lease.** `get` returns a
//!   clone of `T`; eviction can dispose of it while a request still holds that
//!   clone. Per-tenant resources are handle types (a pool, a client) and
//!   disposal is a graceful close — `sqlx`'s `Pool::close()` lets already
//!   acquired connections finish — but a `T` whose disposal is abrupt must
//!   tolerate close-while-cloned, or be kept alive with `keep_forever()`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use r2e_core::type_list::{TCons, TNil};
use r2e_core::plugin::GraphHandle;
use r2e_core::BeanContext;
use tokio::sync::{Notify, OnceCell};
use tokio_util::sync::CancellationToken;

use crate::config::{TenancyConfig, DEFAULT_MAX_ACTIVE, DEFAULT_MAX_NEGATIVE};
use crate::error::{BoxError, TenantError, TenantStatuses};
use crate::source::{ResolutionChain, TenantContext, TenantSource};
use crate::TenantId;

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

/// Every tenant's copy of `T`, created on demand.
pub struct Tenanted<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Clone for Tenanted<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct Inner<T> {
    slots: DashMap<TenantId, Arc<Slot<T>>>,
    negative: DashMap<TenantId, u64>,
    wiring: Wiring<T>,
    /// Time base for the millisecond stamps on slots and negative entries.
    started: Instant,
    /// Bumped by every removal, so a creation that started before one can tell.
    ///
    /// See [`Tenanted::bump_epoch`].
    epoch: AtomicU64,
    trimming: AtomicBool,
    /// Latched by [`Tenanted::drain`]: shutdown started, nothing new is served.
    draining: AtomicBool,
    /// Work that keeps a value alive outside the map, counted so `drain` can
    /// wait for it. See [`Pending`].
    in_flight: AtomicUsize,
    /// Woken every time [`Inner::in_flight`] falls back to zero.
    settled: Notify,
    counters: Counters,
}

struct Slot<T> {
    cell: OnceCell<T>,
    last_used: AtomicU64,
    /// One-shot gate: whoever wins it calls `dispose`, everyone else skips.
    disposed: AtomicBool,
    /// The removal epoch this slot's *current* initialization started at.
    ///
    /// On the slot, not on the resolver, so that every participant sharing this
    /// cell — the task running the initializer and every waiter parked on it —
    /// classifies the one value they share identically. Per-participant captures
    /// let two of them disagree, and the disagreement is not benign: one would
    /// spawn disposal while the other put the same slot back, caching a disposed
    /// resource.
    epoch: AtomicU64,
}

impl<T> Slot<T> {
    fn new(now: u64, epoch: u64) -> Self {
        Self {
            cell: OnceCell::new(),
            last_used: AtomicU64::new(now),
            disposed: AtomicBool::new(false),
            epoch: AtomicU64::new(epoch),
        }
    }

    /// Whether the resource is built (as opposed to a creation in flight).
    fn is_ready(&self) -> bool {
        self.cell.initialized()
    }

    /// Whether the one-shot disposal gate has been taken.
    ///
    /// `true` means the value is dead or being closed right now: nothing may
    /// cache it again.
    fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }

    /// The epoch this initialization started at.
    fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
}

struct Wiring<T> {
    source: Arc<dyn TenantSource<T>>,
    /// The resolved bean graph, filled by the framework after `build_state()`
    /// (or by the embedder). Backs [`TenantContext::bean`] and the cascade,
    /// both of which only run at request time — after the fill.
    ///
    /// The handle is **weak** (this map lives *in* the graph it points at, so
    /// a strong one would be a self-sustaining cycle); the router owns the
    /// graph, so it is alive for every request that can reach us and gone only
    /// after the app is dropped.
    graph: GraphHandle,
    settings: TenantedSettings,
    /// The app-scoped default, when `fallback_to_default()` was asked for.
    fallback: Option<T>,
}

#[derive(Default)]
struct Counters {
    hits: AtomicU64,
    created: AtomicU64,
    create_failures: AtomicU64,
    timeouts: AtomicU64,
    unknown: AtomicU64,
    fallbacks: AtomicU64,
    disposed: AtomicU64,
    evicted_idle: AtomicU64,
    evicted_lru: AtomicU64,
}

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

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// A wired map.
    ///
    /// The [`PerTenant`](crate::PerTenant) plugin is the normal way to get one
    /// (its `build` calls this with the graph handle the framework fills after
    /// `build_state()`); this constructor is for tests and for embedding the
    /// map in something else. `graph` backs [`TenantContext::bean`] and the
    /// cascade — pass [`GraphHandle::default()`] when the source needs
    /// neither, or fill your own handle once your `BeanContext` exists.
    ///
    /// # Panics
    ///
    /// Panics when `settings.max_active` is `0`. A cap of zero would create
    /// every resource and immediately evict it; it is a misconfiguration, not a
    /// way to disable the map.
    #[must_use]
    pub fn new(
        source: Arc<dyn TenantSource<T>>,
        graph: GraphHandle,
        settings: TenantedSettings,
        fallback: Option<T>,
    ) -> Self {
        assert!(
            settings.max_active > 0,
            "`max-active` must be at least 1 for `Tenanted<{}>`: a cap of 0 would create every \
             resource and evict it straight away. Use `PerTenant::max_active(n)` / \
             `tenancy.max-active: n` with n >= 1.",
            std::any::type_name::<T>()
        );
        Self {
            inner: Arc::new(Inner {
                slots: DashMap::new(),
                negative: DashMap::new(),
                wiring: Wiring {
                    source,
                    graph,
                    settings,
                    fallback,
                },
                started: Instant::now(),
                epoch: AtomicU64::new(0),
                trimming: AtomicBool::new(false),
                draining: AtomicBool::new(false),
                in_flight: AtomicUsize::new(0),
                settled: Notify::new(),
                counters: Counters::default(),
            }),
        }
    }

    /// The resource for `tenant`, creating it on first use.
    ///
    /// Concurrent callers for the same cold tenant share one `create` call.
    pub async fn get(&self, tenant: &TenantId) -> Result<T, TenantError> {
        self.resolve(tenant, ResolutionChain::root::<T>()).await
    }

    /// The already-built resource for `tenant`, without creating anything.
    #[must_use]
    pub fn peek(&self, tenant: &TenantId) -> Option<T> {
        let slot = self.inner.slots.get(tenant)?.clone();
        let value = slot.cell.get().cloned();
        if value.is_some() {
            self.touch(&slot);
        }
        value
    }

    /// Tenants with a live resource.
    #[must_use]
    pub fn active(&self) -> Vec<TenantId> {
        self.inner
            .slots
            .iter()
            .filter(|entry| entry.value().is_ready())
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Per-tenant readiness and idle time, including creations in flight.
    #[must_use]
    pub fn stats(&self) -> Vec<TenantStats> {
        let now = self.now_millis();
        self.inner
            .slots
            .iter()
            .map(|entry| TenantStats {
                tenant: entry.key().clone(),
                ready: entry.value().is_ready(),
                idle: Duration::from_millis(
                    now.saturating_sub(entry.value().last_used.load(Ordering::Relaxed)),
                ),
            })
            .collect()
    }

    /// Live counters.
    #[must_use]
    pub fn metrics(&self) -> TenantedMetrics {
        let c = &self.inner.counters;
        TenantedMetrics {
            active: self.active().len(),
            negative: self.inner.negative.len(),
            hits: c.hits.load(Ordering::Relaxed),
            created: c.created.load(Ordering::Relaxed),
            create_failures: c.create_failures.load(Ordering::Relaxed),
            timeouts: c.timeouts.load(Ordering::Relaxed),
            unknown: c.unknown.load(Ordering::Relaxed),
            fallbacks: c.fallbacks.load(Ordering::Relaxed),
            disposed: c.disposed.load(Ordering::Relaxed),
            evicted_idle: c.evicted_idle.load(Ordering::Relaxed),
            evicted_lru: c.evicted_lru.load(Ordering::Relaxed),
        }
    }

    /// The statuses tenancy failures from this map map to.
    #[must_use]
    pub fn statuses(&self) -> TenantStatuses {
        self.settings().statuses
    }

    /// The effective settings.
    #[must_use]
    pub fn settings(&self) -> TenantedSettings {
        self.inner.wiring.settings
    }

    /// Drop a tenant's resource and **await** its disposal.
    ///
    /// Returns `false` when the tenant had nothing **ready**. Use this for a
    /// deliberate teardown (a tenant was offboarded); it is also what the idle
    /// and LRU sweeps call.
    ///
    /// A creation in flight is deliberately left alone (and reported `false`):
    /// detaching an empty slot would let the creation finish into a slot the map
    /// no longer holds, handing the caller a value that is never disposed of.
    /// Evict again once it is ready.
    ///
    /// **It really awaits the closure.** The disposal gate is committed inside
    /// [`take_ready`](Self::take_ready)'s shard-lock critical section, so a
    /// participant that reaches [`reattach`](Self::reattach) a moment later
    /// cannot take the value's disposal off this call and onto a detached task:
    /// it reads the committed gate under that same lock and stands down. The one
    /// case where this returns `true` without awaiting anything is a slot that
    /// was *already* someone else's to close — which no public path can produce,
    /// for the reason spelled out on `take_ready`.
    pub async fn evict(&self, tenant: &TenantId) -> bool {
        let Some(removed) = self.take_ready(tenant) else {
            return false;
        };
        if let Some(debt) = removed.debt {
            self.run_committed_dispose(tenant, &removed.slot, debt).await;
        }
        true
    }

    /// Drop a tenant's cached resource **now**, disposing of it in the
    /// background.
    ///
    /// The synchronous form of [`evict`](Self::evict), for rotation: the next
    /// request rebuilds from the source (a fresh DSN, new credentials) while the
    /// old resource closes behind it. Also clears any negative-cache entry, so a
    /// tenant that was just provisioned is retried immediately.
    ///
    /// Two things the caller has to know, because this cannot await:
    /// - `true` means the resource was **removed and disposal was spawned**, not
    ///   that disposal finished. Use [`evict`](Self::evict) when you need the
    ///   pool to be closed before you return.
    /// - Outside a Tokio runtime there is nothing to spawn on: the resource is
    ///   dropped *without* `dispose` (a `debug!` records it). Every in-process
    ///   caller of `invalidate` is inside the runtime; this only bites in a
    ///   synchronous test harness.
    ///
    /// Like `evict`, a creation in flight is left alone — it stays mapped and
    /// caches what it builds. That creation **overlaps** the invalidation and is
    /// deliberately not fenced: removal never touches it, so it keeps the slot
    /// the map owns. What is fenced off is the opposite case: a creation that was
    /// already **detached** when this ran cannot write its pre-invalidation value
    /// (or its "unknown" verdict) back into the map afterwards.
    ///
    /// # Order
    ///
    /// `take_ready` runs **before** the negative entry is cleared, and never the
    /// other way around. `take_ready` bumps the epoch before it takes the key's
    /// shard lock, so a detached `Ok(None)` writeback either
    ///
    /// - held the shard lock first and inserted its memo — which the
    ///   `negative.remove` below, sequenced after, then clears; or
    /// - takes the lock after the removal, reads the bumped epoch under it, and
    ///   never inserts.
    ///
    /// Clearing first would leave a third case open: the memo lands *after* the
    /// clear and survives the invalidate for a whole `negative-ttl`.
    pub fn invalidate(&self, tenant: &TenantId) -> bool {
        let removed = self.take_ready(tenant);
        self.inner.negative.remove(tenant);
        let Some(removed) = removed else {
            return false;
        };
        if let Some(debt) = removed.debt {
            self.spawn_committed_dispose(tenant, &removed.slot, debt);
        }
        true
    }

    /// Create the resources for `tenants` up front (the plugin's `eager` list).
    ///
    /// Sequential on purpose — a warmup should not open every tenant's pool at
    /// once. Returns the tenants that failed, so a caller can decide whether a
    /// cold tenant is fatal.
    pub async fn preload<I>(&self, tenants: I) -> Vec<(TenantId, TenantError)>
    where
        I: IntoIterator<Item = TenantId>,
    {
        let mut failures = Vec::new();
        for tenant in tenants {
            if let Err(err) = self.get(&tenant).await {
                failures.push((tenant, err));
            }
        }
        failures
    }

    /// Dispose of every cached resource. Called at shutdown.
    ///
    /// Draining **latches**: the map is closed for business afterwards, and
    /// every later `resolve` fails with a `503` rather than opening a resource
    /// nobody will close. Without that, a request arriving mid-shutdown could
    /// repopulate the map behind the drain forever. Creations already in flight
    /// are not aborted — they finish, notice the latch, and dispose of what they
    /// built instead of caching it.
    ///
    /// Removal is conditional on slot identity, so a slot installed by a
    /// concurrent retry is never removed by this drain without being disposed
    /// of, and the pass repeats until no ready slot is left.
    ///
    /// # When this returns, everything is closed
    ///
    /// Walking the map is not enough on its own, because a live value can be
    /// *outside* it while still needing to be closed:
    ///
    /// - a resolve holding a slot that was detached under it (a cancelled
    ///   initializer removes its own empty slot without bumping the epoch; the
    ///   waiter that inherits the cell fills it and only then reattaches), and
    /// - a disposal somebody has committed but not yet awaited — including the
    ///   one that same resolve spawns when it finds itself `Orphaned`.
    ///
    /// Both mint a [`Pending`], so this waits for [`Inner::in_flight`] to reach
    /// zero as well as for the map to come up empty, and re-passes whenever
    /// either is not settled. The counter is read **before** each pass, never
    /// after: a resolve that reattaches at `t1` decrements at `t2 > t1`, so a
    /// count read after the pass could be zero while the slot it just re-mapped
    /// is still there. Read first, walk second, and a zero reading means every
    /// slot that will ever be mapped already was when the pass ran.
    ///
    /// # Termination, and why a flood of 503s cannot hold it open
    ///
    /// `resolve` admits work through a **double check**: it reads the latch
    /// once *without touching the counter*, increments only if that read was
    /// clear, and re-reads before proceeding. So a resolve arriving after the
    /// latch store neither starts work nor counts — it is rejected by the first
    /// read and never appears here. The only resolves that ever increment are
    /// the finite set that was already past that first read when the latch went
    /// up, and each of them either passes the second read (and is then waited
    /// on, by the `SeqCst` argument at the admission site) or decrements again
    /// at once.
    ///
    /// Increments are therefore finite and each is paired with a decrement, so
    /// the counter strictly drains to zero. That is what makes notifying only on
    /// the zero *transition* sufficient: [`Notified`](tokio::sync::Notify) is
    /// registered before the count is read, so a transition after the read wakes
    /// this loop, and a transition before it is either already reflected in the
    /// read or followed by another one — a final state above zero is impossible.
    /// Each in-flight resolve classifies its slot once and each debt is
    /// discharged once, so every pass strictly reduces what is left.
    ///
    /// There is no timeout, deliberately — the rest of `drain` has none either,
    /// and a source's `dispose` is the thing the caller asked to wait for. What
    /// this does *not* cover is a concurrent `invalidate`/`evict` call, which is
    /// not fenced by the latch; racing a manual removal against shutdown is
    /// outside the invariant.
    pub async fn drain(&self) {
        // Latch first, fence second. The latch is the half the termination
        // argument rests on (it is what closes admission — see `resolve` — and
        // it pairs `SeqCst` with the in-flight counter), so it goes up as early
        // as possible. The epoch bump is defence in depth: it turns "reattach,
        // then get drained by the next pass" into "orphan immediately".
        //
        // Both windows between the two are harmless *because of the counter*,
        // not because of any ordering between them — there is none to have
        // across runtime workers, and "no await separates them" was never a
        // serialization argument. Latch-then-fence: a resolve may restore its
        // slot at the old epoch, and a later pass removes it. Fence-then-latch:
        // a resolve may orphan and commit, and that commit mints a debt inside
        // the shard-lock critical section. Either way the work is counted before
        // this can stop looping.
        self.inner.draining.store(true, Ordering::SeqCst);
        self.bump_epoch();
        self.inner.negative.clear();

        loop {
            // Register for the wake-up *before* observing anything, so a
            // counter that hits zero between here and the await is not a lost
            // wakeup.
            let settled = self.inner.settled.notified();
            tokio::pin!(settled);
            settled.as_mut().enable();

            // Before the pass — see the rustdoc.
            let quiet = self.inner.in_flight.load(Ordering::SeqCst) == 0;

            let ready: Vec<(TenantId, Arc<Slot<T>>)> = self
                .inner
                .slots
                .iter()
                .filter(|entry| entry.value().is_ready())
                .map(|entry| (entry.key().clone(), Arc::clone(entry.value())))
                .collect();
            for (tenant, slot) in &ready {
                // `ptr_eq` inside `take_slot`: a recreate's replacement slot is
                // left where it is (and picked up by the next pass) instead of
                // being dropped on the floor undisposed. The gate is taken in
                // the same critical section as the removal.
                if let Some(debt) = self.take_slot(tenant, slot) {
                    self.run_committed_dispose(tenant, slot, debt).await;
                }
            }
            if !ready.is_empty() {
                continue;
            }
            // Nothing mapped, and nothing was in flight when this pass started:
            // whatever was going to be mapped already was, and every committed
            // disposal has completed.
            if quiet && self.inner.in_flight.load(Ordering::SeqCst) == 0 {
                break;
            }
            settled.await;
        }
        self.inner.negative.clear();
    }

    /// Evict idle resources, trim to `max-active`, purge the negative cache.
    ///
    /// What the background [`ServiceComponent`](r2e_core::service::ServiceComponent)
    /// runs on a timer; call it directly from an admin endpoint or a test to
    /// sweep deterministically.
    pub async fn sweep(&self) -> SweepReport {
        let settings = self.settings();
        let mut report = SweepReport {
            negative_purged: self.purge_negative(&settings),
            ..SweepReport::default()
        };

        // `checked_sub`, not `saturating_sub`: a map younger than the TTL has
        // nothing idle in it, and clamping the cutoff to 0 would match slots
        // touched in the map's first millisecond.
        if let Some(cutoff) = settings
            .idle_ttl
            .and_then(|ttl| self.now_millis().checked_sub(ttl.as_millis() as u64))
        {
            for tenant in self.idle_since(cutoff) {
                if self.evict(&tenant).await {
                    report.idle_evicted += 1;
                    self.inner.counters.evicted_idle.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        report.lru_evicted = self.trim_to_max_active(&settings).await;
        report
    }

    /// Write a negative-cache entry for `tenant`, bypassing every ownership and
    /// epoch rule.
    ///
    /// A test seam, not API. "A ready slot and a fresh negative entry for the
    /// same tenant" is precisely the state
    /// [`remember_negative_owned`](Self::remember_negative_owned) exists to
    /// prevent, so the test that pins the *other* half of the defence —
    /// `resolve` consulting the ready slot before the memo, which is what keeps
    /// a live tenant up if that first rule ever regresses — has no honest way to
    /// reach it. Rather than let the two rules mask each other, this reaches in.
    #[doc(hidden)]
    pub fn force_negative_entry(&self, tenant: &TenantId) {
        self.inner.negative.insert(tenant.clone(), self.now_millis());
    }

    /// A test seam, not API. Removes and disposes of `tenant`'s slot and then
    /// asks [`reattach`](Self::reattach) to put that same slot back, with the
    /// epoch rule deliberately neutralised (the slot is re-stamped with the
    /// *current* epoch) so that only the disposal gate can refuse it.
    ///
    /// Returns `true` when the reattach was refused.
    #[doc(hidden)]
    pub async fn force_reattach_after_dispose(&self, tenant: &TenantId) -> bool {
        let Some(removed) = self.take_ready(tenant) else {
            return false;
        };
        if let Some(debt) = removed.debt {
            self.run_committed_dispose(tenant, &removed.slot, debt).await;
        }
        removed.slot.epoch.store(self.epoch(), Ordering::SeqCst);
        matches!(
            self.reattach(tenant, &removed.slot),
            SlotOwnership::Orphaned { .. }
        )
    }

    /// A test seam, not API. Replays the remover-against-a-late-participant race
    /// in a chosen order.
    ///
    /// The participant is one that got its value out of `get_or_try_init` just
    /// as a removal ran, and reaches [`reattach`](Self::reattach) either side of
    /// it. The remover half is what [`evict`](Self::evict) does — `take_ready`
    /// (bump, remove and commit the gate under the key's shard lock) then the
    /// `dispose` await — spelled out here so the participant can be spliced into
    /// the window between the two, which no concurrent test can reach: it is
    /// await-free.
    ///
    /// - `remove_first`: returns `(true, true)` when the remover owned the
    ///   disposal and the late participant **lost** the gate, spawning nothing —
    ///   i.e. the value was really closed by the time the remover returned.
    /// - `!remove_first`: the participant restores the slot first; returns
    ///   `(true, true)` when it restored and the remover then removed *and*
    ///   owned the disposal of the re-mapped slot.
    #[doc(hidden)]
    pub async fn force_remove_race(&self, tenant: &TenantId, remove_first: bool) -> (bool, bool) {
        if remove_first {
            // The participant's handle, taken while the slot is still the map's:
            // exactly what an initialization that returned a moment too early
            // is holding.
            let Some(shared) = self.inner.slots.get(tenant).map(|e| Arc::clone(e.value())) else {
                return (false, false);
            };
            let Some(removed) = self.take_ready(tenant) else {
                return (false, false);
            };
            let late = self.reattach(tenant, &shared);
            let owed = removed.debt.is_some();
            if let Some(debt) = removed.debt {
                self.run_committed_dispose(tenant, &removed.slot, debt).await;
            }
            (
                owed,
                matches!(late, SlotOwnership::Orphaned { debt: None }),
            )
        } else {
            // Detach the ready slot *without* taking its gate — the state a
            // participant holding a live, unmapped slot is in.
            self.bump_epoch();
            let Some((_, shared)) = self.inner.slots.remove_if(tenant, |_, slot| slot.is_ready())
            else {
                return (false, false);
            };
            shared.epoch.store(self.epoch(), Ordering::SeqCst);
            let restored = matches!(self.reattach(tenant, &shared), SlotOwnership::Restored);
            let owed = match self.take_ready(tenant) {
                Some(removed) => match removed.debt {
                    Some(debt) => {
                        self.run_committed_dispose(tenant, &removed.slot, debt).await;
                        true
                    }
                    None => false,
                },
                None => false,
            };
            (restored, owed)
        }
    }

    /// A test seam, not API. Replays the drain-against-a-late-participant race
    /// on a **detached** slot, in a chosen order.
    ///
    /// The shape is the one that made the old lock-free
    /// [`take_slot`](Self::take_slot) fallback unsound: a participant `P` holds a
    /// live slot that is no longer under the key (its initializer's competitor
    /// vanished, or it was detached), while a drain-side caller `A` reaches
    /// `take_slot` for that same slot. `A` therefore takes the branch where the
    /// key does **not** hold its slot — vacant, or holding somebody else's. The
    /// slot is re-stamped with the current epoch so the epoch rule cannot answer
    /// for the gate rule; only the under-guard commit can.
    ///
    /// - `!restore_first`: `A` runs first and finds the key **vacant** — the
    ///   fallback shape. Returns `(true, true)` when `A` owed the disposal and
    ///   `P`'s later `reattach` refused to restore, spawning nothing.
    /// - `restore_first`: `P` restores the slot first, so `A` finds the key
    ///   **occupied by that very slot**. Returns `(true, true)` when `A` owed the
    ///   disposal and the key is empty afterwards.
    ///
    /// Either way the debt `A` takes on is awaited before this returns, exactly
    /// as `drain` and `detach_and_dispose` await theirs.
    #[doc(hidden)]
    pub async fn force_take_slot_race(&self, tenant: &TenantId, restore_first: bool) -> (bool, bool) {
        // Detach the ready slot *without* taking its gate: a live value held by
        // a participant, unmapped.
        self.bump_epoch();
        let Some((_, shared)) = self.inner.slots.remove_if(tenant, |_, slot| slot.is_ready()) else {
            return (false, false);
        };
        shared.epoch.store(self.epoch(), Ordering::SeqCst);

        if restore_first {
            let restored = matches!(self.reattach(tenant, &shared), SlotOwnership::Restored);
            let debt = self.take_slot(tenant, &shared);
            let owed = debt.is_some();
            if let Some(debt) = debt {
                self.run_committed_dispose(tenant, &shared, debt).await;
            }
            (owed, restored && self.inner.slots.get(tenant).is_none())
        } else {
            let debt = self.take_slot(tenant, &shared);
            let owed = debt.is_some();
            let late = self.reattach(tenant, &shared);
            if let Some(debt) = debt {
                self.run_committed_dispose(tenant, &shared, debt).await;
            }
            (owed, matches!(late, SlotOwnership::Orphaned { debt: None }))
        }
    }

    /// A test seam, not API. Replays the two-participant race
    /// [`reattach`](Self::reattach) exists to close, in a chosen order.
    ///
    /// The shape: one ready slot `S` shared by two participants, a competing
    /// empty slot `S2` installed under the key, and `S2` disappearing when its
    /// own initializer fails — a cleanup that deliberately does not bump the
    /// epoch. `S` is re-stamped with the current epoch so that the epoch rule
    /// cannot answer for the gate rule; only the under-lock gate commit can.
    ///
    /// - `orphan_first`: participant 1 classifies against `S2` (→ `Orphaned`,
    ///   gate committed inline), `S2` vanishes, participant 2 then finds a
    ///   vacant key at a matching epoch. Returns `(true, true)` when P1 owed the
    ///   disposal and P2 **refused** to restore.
    /// - `!orphan_first`: `S2` vanishes first, participant 2 restores `S`, and
    ///   participant 1 then finds the key holding `S` itself. Returns
    ///   `(true, true)` when P2 restored and P1 answered `Kept` — spawning
    ///   nothing.
    ///
    /// Whatever the order, the disposal debt this seam creates is discharged
    /// before it returns, so a test can assert on `disposals()` directly.
    #[doc(hidden)]
    pub async fn force_reattach_race(&self, tenant: &TenantId, orphan_first: bool) -> (bool, bool) {
        // Detach the ready slot *without* taking its gate: this seam is about
        // two participants sharing a live value, not about a removal.
        self.bump_epoch();
        let Some((_, shared)) = self.inner.slots.remove_if(tenant, |_, slot| slot.is_ready())
        else {
            return (false, false);
        };
        // The competing empty slot, still initializing as far as the map knows.
        let competitor = self.slot_for(tenant);
        shared.epoch.store(self.epoch(), Ordering::SeqCst);

        // The competitor's initializer failed or was cancelled: its cleanup
        // removes the empty slot and, by design, bumps nothing.
        let drop_competitor = || {
            self.inner.slots.remove_if(tenant, |_, current| {
                Arc::ptr_eq(current, &competitor) && !current.is_ready()
            });
        };

        if orphan_first {
            let p1 = self.reattach(tenant, &shared);
            drop_competitor();
            let p2 = self.reattach(tenant, &shared);
            let owed = match p1 {
                SlotOwnership::Orphaned { debt: Some(debt) } => {
                    // Discharge the debt P1 took on, so the test sees a settled
                    // world rather than a value gated shut and never closed.
                    self.run_committed_dispose(tenant, &shared, debt).await;
                    true
                }
                _ => false,
            };
            (owed, !matches!(p2, SlotOwnership::Restored))
        } else {
            drop_competitor();
            let p2 = self.reattach(tenant, &shared);
            let p1 = self.reattach(tenant, &shared);
            // Nothing was committed here — `Kept` takes no gate — so nothing is
            // awaited: the slot is back in the map, alive.
            (
                matches!(p2, SlotOwnership::Restored),
                matches!(p1, SlotOwnership::Kept),
            )
        }
    }

    // ── internals ───────────────────────────────────────────────────────────

    pub(crate) async fn resolve(
        &self,
        tenant: &TenantId,
        chain: ResolutionChain,
    ) -> Result<T, TenantError> {
        let wiring = &self.inner.wiring;

        // Admission is **double-checked**, and the order of the three steps is
        // the whole point. This first read MUST stay ahead of `Pending::new`:
        // moving it after (or dropping it, since the second read looks
        // redundant) reintroduces the starvation below, and no test can see the
        // difference — the rejected path mints and drops its guard without an
        // await in between, so the counter is back down before `resolve`
        // returns and the damage is only ever visible as a livelock under
        // sustained overlap. The structure carries this one.
        //
        // The first read touches no shared counter. Every request that arrives
        // after the latch store fails it and leaves without ever incrementing —
        // which is what keeps a flood of post-shutdown 503s from holding the
        // counter permanently above zero and starving `drain` forever. (Under
        // the plugin lifecycle the listener is still accepting while this hook
        // runs, so that flood is an ordinary production shape, not a contrived
        // one.)
        if self.is_draining() {
            return Err(draining_error(tenant));
        }

        // Only now is the work counted, and only a resolve that passes the
        // *second* read is admitted: it is dropped again immediately otherwise.
        // In the `SeqCst` total order an admitted resolve has
        //
        //     increment  <  re-check  <  latch store  <  drain's counter read
        //
        // (the re-check read `false`, and the latch only ever goes `false` ->
        // `true`, so a load that reads `false` precedes every `true` store in
        // the total order — including this `drain`'s, which in turn precedes
        // its own counter read by program order; and the increment precedes the
        // re-check by program order). So `drain` either sees this increment and
        // waits for it, or this resolve had already decremented — in which case
        // it finished classifying its slot before the read, and the pass that
        // follows sees whatever it left mapped. Never both missing.
        //
        // The transient incrementers are therefore exactly the finite set that
        // was already past the first read when the latch went up: the counter
        // strictly drains, which is what makes `drain`'s notify-on-zero wake up.
        let _in_flight = Pending::new(&self.inner);
        if self.is_draining() {
            return Err(draining_error(tenant));
        }

        // Before the negative cache, not after: a live resource always beats a
        // negative memo. The two can coexist — a creation that finished into a
        // *detached* cell can report the tenant unknown after another slot
        // cached a real value under the same key — and honouring the memo there
        // would shadow a working tenant (404 or fallback) for a whole
        // `negative-ttl`.
        if let Some(value) = self.hit(tenant) {
            return Ok(value);
        }

        if self.negative_hit(tenant, &wiring.settings) {
            return self.unknown_or_fallback(tenant, wiring);
        }

        let slot = self.slot_for(tenant);
        self.touch(&slot);
        if let Some(value) = slot.cell.get() {
            self.inner.counters.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(value.clone());
        }

        // The `Arc<Slot>` is already cloned out of the map: no shard guard is
        // held across the awaits below.
        let outcome = slot
            .cell
            .get_or_try_init(|| async {
                // The guard lives *inside* the initializer, so only the task
                // that actually runs a creation arms it. A waiter parked on the
                // cell never enters this closure: cancelling it must not touch
                // the map, or it would detach the slot of the creation it was
                // waiting for (whose value would then never be disposed of).
                //
                // What it covers is the two paths that leave the initializer
                // without returning a value — a panic inside
                // `TenantSource::create`, and this future being dropped
                // mid-creation (a client disconnect, an outer timeout) — either
                // of which would otherwise leave an empty slot mapped with
                // nobody to retry it, one leaked entry per hostile tenant id.
                // The error path disarms it and lets `resolve` below own the
                // removal.
                let mut cleanup = EmptySlotGuard {
                    inner: &self.inner,
                    tenant,
                    slot: &slot,
                    armed: true,
                };
                // Stamp the slot with the epoch *this* initialization starts at,
                // before the source is asked. Two writes can land here: the
                // resolver that created the slot wrote its own reading in
                // `slot_for`, and whoever ends up running the initializer (this
                // task — possibly a waiter that inherited the cell after a
                // failure, seconds later) overwrites it now. This one wins,
                // because it is the one that actually brackets the `create`
                // call; the earlier write is only ever *older*, which would
                // over-fence rather than under-fence, so losing the race in the
                // other direction would be safe too. Initializers never run
                // concurrently on one cell, so there is no third case.
                slot.epoch.store(self.epoch(), Ordering::SeqCst);
                let outcome = async {
                    // First thing inside the initializer, not just before it:
                    // the waiters that queued behind a failed init run this
                    // closure in turn, and this is where they see the negative
                    // entry the first attempt wrote. Without it, an
                    // unknown-tenant wave calls the directory once per waiter.
                    if self.negative_hit(tenant, &wiring.settings) {
                        return Err(CreateFailure::Unknown);
                    }
                    let ctx = TenantContext::new(tenant, wiring.graph.clone(), chain);
                    let creating = wiring.source.create(tenant, &ctx);
                    let created = match wiring.settings.create_timeout {
                        Some(budget) => match tokio::time::timeout(budget, creating).await {
                            Ok(created) => created,
                            Err(_) => {
                                self.inner.counters.timeouts.fetch_add(1, Ordering::Relaxed);
                                return Err(CreateFailure::Failed(TenantError::Timeout(
                                    tenant.clone(),
                                )));
                            }
                        },
                        None => creating.await,
                    };
                    match created {
                        Ok(Some(value)) => {
                            self.inner.negative.remove(tenant);
                            self.inner.counters.created.fetch_add(1, Ordering::Relaxed);
                            Ok(value)
                        }
                        Ok(None) => {
                            self.inner.counters.unknown.fetch_add(1, Ordering::Relaxed);
                            self.remember_negative_owned(tenant, &slot, &wiring.settings);
                            Err(CreateFailure::Unknown)
                        }
                        Err(cause) => {
                            self.inner
                                .counters
                                .create_failures
                                .fetch_add(1, Ordering::Relaxed);
                            Err(CreateFailure::Failed(classify(tenant, cause)))
                        }
                    }
                }
                .await;
                // Both outcomes are handled by `resolve`: `Ok` by the reattach
                // below, `Err` by the removal below. One owner per path.
                cleanup.armed = false;
                outcome
            })
            .await;

        match outcome {
            Ok(value) => {
                let value = value.clone();
                if self.is_draining() {
                    // Shutdown started while this creation was in flight. The
                    // value is real and cached, so it has to be disposed of
                    // rather than handed out: `drain` may already have walked
                    // past this slot. The per-slot gate keeps that from being a
                    // double dispose if it has not.
                    self.detach_and_dispose(tenant, &slot).await;
                    return Err(draining_error(tenant));
                }

                // Self-heal. This cell may have been detached from the map
                // while the creation ran: an earlier attempt on it was
                // cancelled (its guard removed the slot) or failed (this
                // `resolve`'s `Err` arm removed it), and this task inherited the
                // cell as a waiter. Putting the slot back is what makes the
                // value the map's again — and therefore disposable.
                let ownership = self.reattach(tenant, &slot);
                if !matches!(ownership, SlotOwnership::Kept) {
                    // The initializer clears the negative entry on success, but
                    // a *racing* attempt for the same tenant may have written
                    // one after that. A success wins.
                    self.inner.negative.remove(tenant);
                }
                // A concurrent resolve recreated the key while this creation ran
                // (or a removal fenced this one off), and this value belongs to
                // nobody. `reattach` already committed the slot's gate under the
                // shard lock; a `Some` debt says it was *this* caller that won
                // it, in which case only the await is left to spawn (`None`:
                // somebody else owns it, spawn nothing). The value is still
                // handed out — `get` never handed out a lease, so
                // close-while-cloned is already part of the contract.
                //
                // This runs before the drain re-check below on purpose. The gate
                // is already taken, so `detach_and_dispose` there would find it
                // committed and await nothing — the debt has to be discharged
                // first.
                if let SlotOwnership::Orphaned { debt: Some(debt) } = ownership {
                    self.spawn_committed_dispose(tenant, &slot, debt);
                }
                if self.is_draining() {
                    // The latch went up between the check above and the
                    // reattach: undo it, or a slot put back behind `drain`'s
                    // last pass would stay mapped and never be disposed of.
                    self.detach_and_dispose(tenant, &slot).await;
                    return Err(draining_error(tenant));
                }
                self.touch(&slot);
                self.enforce_max_active(&wiring.settings);
                Ok(value)
            }
            Err(failure) => {
                // Nothing is cached on failure, and this arm is the only owner
                // of that removal (the guard disarmed itself). Two conditions:
                // `ptr_eq` keeps a concurrent retry's fresh slot from being
                // removed by this one's cleanup, and `is_ready` keeps a waiter
                // that already succeeded *on this very cell* from having its
                // value detached — both are evaluated under the shard lock.
                self.inner.slots.remove_if(tenant, |_, current| {
                    Arc::ptr_eq(current, &slot) && !current.is_ready()
                });
                match failure {
                    CreateFailure::Failed(err) => Err(err),
                    CreateFailure::Unknown => self.unknown_or_fallback(tenant, wiring),
                }
            }
        }
    }

    /// Put `slot` back under `tenant` after a creation that may have been
    /// detached, reporting who owns the key.
    ///
    /// The whole decision happens under one shard lock, so a concurrent
    /// `slot_for` either loses the key to this slot or is already the occupant.
    ///
    /// Filling a vacant key takes two things beyond the lock, because a vacant
    /// key reads the same whether nobody ever mapped this tenant or an
    /// [`invalidate`](Self::invalidate) just emptied it:
    ///
    /// - **the epoch has not moved** since this *initialization* started
    ///   (`slot.epoch()` — one reading shared by every participant on this cell,
    ///   rather than a per-caller capture). See [`bump_epoch`](Self::bump_epoch)
    ///   for why the shard lock makes that reading trustworthy.
    /// - **the disposal gate is untaken.** A slot whose gate is committed holds
    ///   a value that is closed or closing; putting it back would cache a dead
    ///   resource.
    ///
    /// # Why the gate is committed *here*, inline
    ///
    /// Two participants can share one cell and still classify its value
    /// differently — not because they read different epochs (they read one, off
    /// the slot), but because the *key* changes underneath them. A competing
    /// empty slot appears (one participant orphans against it) and then vanishes
    /// when its own initializer fails or is cancelled — and that cleanup
    /// deliberately does **not** bump the epoch, or the legitimate
    /// waiter-inherits-and-retries reattach could never happen. The next
    /// participant then finds a vacant key at a matching epoch: exactly the
    /// state that says "restore".
    ///
    /// So orphaning does not merely *schedule* a disposal, it **commits the
    /// slot's one-shot gate right here, under the entry guard**, and reports
    /// whether it won it. That puts the commit and the restore decision in the
    /// same critical section, and the shard lock does the rest:
    ///
    /// | Order under the key's shard lock | What the other participant sees | Outcome |
    /// |---|---|---|
    /// | orphan first | vacant + matching epoch, but `is_disposed()` — read under the same lock | refuses, `Orphaned` with the gate already lost: it spawns nothing |
    /// | restore first | the key occupied by **this very slot** | `ptr_eq` → `Kept`: no gate commit, no disposal |
    ///
    /// Either way exactly one participant owns the disposal, and a disposed
    /// value is never the map's. Committing inside the spawned disposal task
    /// instead would leave the window open for the whole scheduling delay.
    ///
    /// # And against a concurrent public removal
    ///
    /// | Removal reaches the shard lock | This restore reads | Outcome |
    /// |---|---|---|
    /// | after this restore | old epoch → inserted | the removal's `take_ready` then finds a **ready** slot under the key and removes and disposes of it: the removal still wins |
    /// | before this restore | bumped epoch (lock handoff orders the read after the bump) | `Orphaned`, and the CAS is **lost**: `take_ready` committed the gate inside the very critical section that removed the slot, so this caller spawns nothing and the remover's own `await` still closes the value |
    /// | not at all (unrelated key) | possibly bumped anyway | `Orphaned` at worst — a false fence costs one rebuild |
    ///
    /// The `debt` the `Orphaned` arms carry is the gate's verdict:
    /// `true` means this caller — and nobody else — must reach
    /// [`run_committed_dispose`](Self::run_committed_dispose).
    fn reattach(&self, tenant: &TenantId, slot: &Arc<Slot<T>>) -> SlotOwnership<T> {
        match self.inner.slots.entry(tenant.clone()) {
            Entry::Vacant(vacant) => {
                if self.epoch() == slot.epoch() && !slot.is_disposed() {
                    vacant.insert(Arc::clone(slot));
                    return SlotOwnership::Restored;
                }
                // Stale by construction: something was removed while this
                // creation ran (or the value is already being closed) and this
                // key is empty. Orphan it — disposed of, still handed to its
                // caller. The `commit_dispose` also *closes* the window above
                // for whoever comes next, and MUST stay inside this entry guard
                // to do so.
                SlotOwnership::Orphaned {
                    debt: self.commit_dispose(slot),
                }
            }
            Entry::Occupied(occupied) => {
                if Arc::ptr_eq(occupied.get(), slot) {
                    SlotOwnership::Kept
                } else {
                    // MUST stay inside this entry guard — see `commit_dispose`.
                    SlotOwnership::Orphaned {
                        debt: self.commit_dispose(slot),
                    }
                }
            }
        }
    }

    /// Unmap the slot (only when it is still this one) and dispose of its value.
    ///
    /// The gate is taken under the shard lock by
    /// [`take_slot`](Self::take_slot); losing it means someone else owns the
    /// await and this caller must not duplicate it.
    async fn detach_and_dispose(&self, tenant: &TenantId, slot: &Arc<Slot<T>>) {
        if let Some(debt) = self.take_slot(tenant, slot) {
            self.run_committed_dispose(tenant, slot, debt).await;
        }
    }

    /// Spawn the await half of a disposal this caller already committed.
    ///
    /// Outside a Tokio runtime there is nothing to spawn on. The gate stays
    /// taken: the slot is already out of the map (or was never the map's), so
    /// nobody can reach it to retry, and a second commit would only risk a
    /// double dispose the moment a runtime does exist.
    fn spawn_committed_dispose(&self, tenant: &TenantId, slot: &Arc<Slot<T>>, debt: DisposalDebt<T>) {
        let this = self.clone();
        let disposing = tenant.clone();
        let slot = Arc::clone(slot);
        // The debt moves into the future: it keeps the work counted until the
        // spawned task finishes, and if there is nothing to spawn on it is
        // dropped with the future, so the counter never gets stuck.
        if !spawn_detached(async move {
            this.run_committed_dispose(&disposing, &slot, debt).await;
        }) {
            tracing::debug!(
                tenant = %tenant,
                resource = std::any::type_name::<T>(),
                "no Tokio runtime to dispose on: dropping the resource without dispose"
            );
        }
    }

    fn unknown_or_fallback(&self, tenant: &TenantId, wiring: &Wiring<T>) -> Result<T, TenantError> {
        match &wiring.fallback {
            Some(default) => {
                self.inner.counters.fallbacks.fetch_add(1, Ordering::Relaxed);
                Ok(default.clone())
            }
            None => Err(TenantError::Unknown(tenant.clone())),
        }
    }

    /// The mapped, **ready** value for `tenant`, counted as a cache hit.
    ///
    /// [`resolve`](Self::resolve) runs this ahead of the negative cache, so a
    /// value the map actually holds can never be shadowed by a stale "unknown"
    /// memo — and the memo is dropped on the way out, so the next request does
    /// not pay for this check either.
    fn hit(&self, tenant: &TenantId) -> Option<T> {
        let slot = self
            .inner
            .slots
            .get(tenant)
            .map(|entry| Arc::clone(entry.value()))?;
        let value = slot.cell.get()?.clone();
        self.touch(&slot);
        self.inner.counters.hits.fetch_add(1, Ordering::Relaxed);
        // `contains_key` first: a read lock on the shard, where the `remove`
        // this almost always skips would take a write lock per cache hit.
        if self.inner.negative.contains_key(tenant) {
            self.inner.negative.remove(tenant);
        }
        Some(value)
    }

    /// Remember `tenant` as unknown — but only on the word of an attempt that
    /// still speaks for the key.
    ///
    /// A *detached* attempt speaks for nobody: another slot may already hold a
    /// real value for this tenant, and remembering "unknown" over it would
    /// shadow a working tenant for a whole `negative-ttl`. A vacant key is only
    /// this attempt's own cleanup when nothing was removed since it started —
    /// hence the same epoch fence as [`reattach`](Self::reattach), so an
    /// `Ok(None)` from before an `invalidate` cannot repopulate the negative
    /// cache that `invalidate` just cleared.
    ///
    /// The test and the insert are one critical section, under the `slots` shard
    /// lock: split, a fresh resolve could install its slot in between and then
    /// be aborted by this entry at the negative re-check *inside* its own
    /// initializer, without the source ever being asked.
    ///
    /// What the fence deliberately does **not** cover: a creation that is still
    /// *mapped* (the occupied branch) writes its memo even if an `invalidate`
    /// ran while it was in flight. Removal only ever touches ready slots, so
    /// that creation was never removed; it **overlaps** the invalidation and is
    /// deliberately not fenced. Only its completion is concurrent — the answer
    /// the source gave it may well predate the invalidation — but a request
    /// arriving right after the invalidation would have asked the source at the
    /// same moment and got the same thing, so there is nothing better to do.
    ///
    /// # Lock order
    ///
    /// This is the only place that touches `negative` while holding a `slots`
    /// guard, and it fixes the order **`slots` → `negative`** for the whole
    /// file. No path may take a `slots` guard while holding a `negative` one.
    /// Bounding the cache is deliberately left outside the guard: it walks every
    /// negative shard and needs none of this atomicity.
    fn remember_negative_owned(
        &self,
        tenant: &TenantId,
        slot: &Arc<Slot<T>>,
        settings: &TenantedSettings,
    ) {
        if settings.negative_ttl.is_none() || settings.max_negative == 0 {
            return;
        }
        let now = self.now_millis();
        {
            // `entry` is *bound*, not matched-and-dropped: the guard has to
            // outlive the insert below, or the ownership test and the write are
            // two separate critical sections again and everything between them
            // (a fresh resolve installing its slot, a removal bumping the epoch)
            // slips into the gap.
            let entry = self.inner.slots.entry(tenant.clone());
            let owned = match &entry {
                Entry::Occupied(occupied) => Arc::ptr_eq(occupied.get(), slot),
                Entry::Vacant(_) => self.epoch() == slot.epoch(),
            };
            if owned {
                self.inner.negative.insert(tenant.clone(), now);
            }
            drop(entry);
            if !owned {
                return;
            }
        }
        self.bound_negative(tenant, settings);
    }

    /// How many tenants have a **built** resource.
    ///
    /// What `max-active` is really about: `slots.len()` also counts creations in
    /// flight, which no trim can evict.
    fn ready_count(&self) -> usize {
        self.inner
            .slots
            .iter()
            .filter(|entry| entry.value().is_ready())
            .count()
    }

    fn slot_for(&self, tenant: &TenantId) -> Arc<Slot<T>> {
        // Clone the `Arc` out and drop the guard before returning: every caller
        // awaits, and awaiting under a DashMap guard deadlocks the shard.
        if let Some(existing) = self.inner.slots.get(tenant) {
            return Arc::clone(existing.value());
        }
        let now = self.now_millis();
        let epoch = self.epoch();
        Arc::clone(
            self.inner
                .slots
                .entry(tenant.clone())
                .or_insert_with(|| Arc::new(Slot::new(now, epoch)))
                .value(),
        )
    }

    /// Remove a tenant's slot, but only once its resource is built — and take
    /// its disposal gate in the same breath.
    ///
    /// The one removal primitive every caller goes through, so "an in-flight
    /// creation is never detached from the map" holds for eviction, the sweeps
    /// and `invalidate` alike — and so does the epoch fence, which is bumped
    /// here for all of them.
    ///
    /// # Why the gate is committed inside the predicate
    ///
    /// `remove_if`'s predicate runs **under the key's shard lock**, which is the
    /// same lock [`reattach`](Self::reattach) decides under. Committing there
    /// rather than after the lock is released is what keeps an awaited remover
    /// from losing its own value: a late participant that reaches `reattach`
    /// afterwards reads the committed gate under that lock and spawns nothing,
    /// so `evict().await` really has closed the resource by the time it returns.
    /// Deferring the commit to `dispose` would leave a window where the
    /// participant wins the CAS, detaches the disposal onto a spawned task, and
    /// the remover returns early.
    ///
    /// # The already-committed edge
    ///
    /// A **mapped** slot cannot have a committed gate. The argument is over the
    /// six commit sites rather than over intentions: each one holds this key's
    /// shard lock, and none of them ends its critical section with the slot it
    /// committed still under the key.
    ///
    /// - `take_ready` here and `take_slot`'s occupied-and-ours branch commit and
    ///   **remove** in the same section;
    /// - `take_slot`'s vacant and occupied-by-another branches, and both of
    ///   `reattach`'s `Orphaned` arms, commit a slot that is *not* the one under
    ///   the key (there is none, or it is a different `Arc`).
    ///
    /// And the only way a slot becomes mapped again is `reattach`'s vacant
    /// restore, which reads `is_disposed()` under this same guard and refuses.
    /// So the two states never meet.
    ///
    /// Should it happen anyway — a test seam, or future code — the slot is still
    /// removed (it is dying either way) and the disposal is **skipped**: the
    /// committer owns that await, and a remover cannot await a disposal it does
    /// not own. Logged at debug rather than asserted, because the safe action
    /// and the loud action differ here and the safe one wins.
    fn take_ready(&self, tenant: &TenantId) -> Option<Removed<T>> {
        self.bump_epoch();
        let mut debt = None;
        let (_, slot) = self.inner.slots.remove_if(tenant, |_, slot| {
            if !slot.is_ready() {
                return false;
            }
            // MUST stay inside this predicate: it runs under the key's shard
            // lock, which is the only thing ordering it against a `reattach`.
            debt = self.commit_dispose(slot);
            true
        })?;
        if debt.is_none() {
            tracing::debug!(
                tenant = %tenant,
                resource = std::any::type_name::<T>(),
                "removed a slot whose disposal was already owned by someone else"
            );
        }
        Some(Removed { slot, debt })
    }

    /// Unmap `slot` when it is still the one under `tenant`, committing its
    /// disposal gate in the same critical section. Returns whether **this**
    /// caller owes the value its `dispose` await.
    ///
    /// The identity-conditional twin of [`take_ready`](Self::take_ready), for
    /// the paths that already hold the slot they mean to remove: `drain` and the
    /// draining escape in `resolve`. Unlike `take_ready` it also commits a slot
    /// that is *no longer mapped* — the caller is holding a value that has to be
    /// closed either way — and that is exactly why the whole thing goes through
    /// `entry()` rather than `remove_if` plus a bare CAS.
    ///
    /// # Every branch commits under the key's entry guard
    ///
    /// [`reattach`](Self::reattach) also goes through `entry()` on this key, so
    /// the guard is the serialization point between "this value is being closed"
    /// and "this value is going back into the map":
    ///
    /// - **occupied by this slot** — commit and remove in one section. The
    ///   remover owns both, and no reattach can interleave.
    /// - **vacant** — commit while still *holding* the vacant guard. A reattach
    ///   arriving afterwards takes the same guard, reads `is_disposed()` and
    ///   refuses; one arriving before has re-mapped the slot, which is the
    ///   occupied case above. Dropping the guard before the CAS is what let a
    ///   restore slip in between and steal the disposal onto a detached task.
    /// - **occupied by a different slot** — the key is somebody else's, so
    ///   nothing is removed, but this slot's gate is still committed under the
    ///   guard: every reattach of *this* slot targets *this* key, so it is
    ///   serialized all the same.
    fn take_slot(&self, tenant: &TenantId, slot: &Arc<Slot<T>>) -> Option<DisposalDebt<T>> {
        match self.inner.slots.entry(tenant.clone()) {
            Entry::Occupied(occupied) => {
                if Arc::ptr_eq(occupied.get(), slot) {
                    // MUST stay inside this critical section — see the rustdoc.
                    let debt = self.commit_dispose(slot);
                    occupied.remove();
                    debt
                } else {
                    // MUST stay inside this critical section — see the rustdoc.
                    self.commit_dispose(slot)
                }
            }
            Entry::Vacant(vacant) => {
                // MUST stay inside this critical section: the guard is bound so
                // that it outlives the CAS, exactly as in
                // `remember_negative_owned`.
                let debt = self.commit_dispose(slot);
                drop(vacant);
                debt
            }
        }
    }

    fn is_draining(&self) -> bool {
        // `SeqCst`, paired with the `SeqCst` increment in [`Pending::new`]: the
        // two sides are a store-buffer shape (drain stores the latch then reads
        // the counter; a resolve increments the counter then reads the latch),
        // and release/acquire lets *both* of them miss. See `drain`.
        self.inner.draining.load(Ordering::SeqCst)
    }

    /// Take the slot's one-shot disposal gate. `Some` means **this caller** owns
    /// the disposal and owes the value a [`run_committed_dispose`].
    ///
    /// # Invariant: never called outside a shard-lock critical section
    ///
    /// Every call site holds the tenant key's `slots` shard lock — either a
    /// `remove_if` predicate or a bound `Entry` guard — because that lock is the
    /// only thing that orders a commit against a concurrent
    /// [`reattach`](Self::reattach) of the same slot. There are six, and they
    /// are the whole list: [`take_ready`](Self::take_ready) (in its predicate),
    /// [`take_slot`](Self::take_slot) (all three entry branches) and `reattach`
    /// (both `Orphaned` arms). A seventh, anywhere outside a guard, reopens the
    /// window this split exists to close. The tests cannot catch a CAS moved to
    /// just *after* its lock — nothing schedulable separates the two — so the
    /// comment at each site is the guard rail.
    ///
    /// The two halves must also stay paired: every `Some` reaches the await
    /// exactly once, or a value is gated shut and never closed. The
    /// [`DisposalDebt`] is what pairs them — it is minted here, inside the
    /// critical section, so `drain` cannot observe the slot leave the map before
    /// the work is counted, and it is only discharged by being dropped.
    ///
    /// Slots with nothing in them (an initialization that never completed)
    /// report `None`: there is nothing to hand to a source, and pretending
    /// otherwise would leave a caller owing an await that cannot do anything.
    fn commit_dispose(&self, slot: &Slot<T>) -> Option<DisposalDebt<T>> {
        if slot.cell.get().is_none() {
            return None;
        }
        slot.disposed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            .then(|| DisposalDebt::new(&self.inner))
    }

    /// Run the disposal this caller already won with [`commit_dispose`].
    ///
    /// Takes the debt by value and drops it on the way out, so the work stops
    /// being counted exactly when it is done — including when `dispose` panics
    /// or this future is dropped mid-await.
    async fn run_committed_dispose(&self, tenant: &TenantId, slot: &Slot<T>, debt: DisposalDebt<T>) {
        let _debt = debt;
        let Some(value) = slot.cell.get() else {
            return;
        };
        self.inner.counters.disposed.fetch_add(1, Ordering::Relaxed);
        self.inner.wiring.source.dispose(tenant, value.clone()).await;
    }

    fn now_millis(&self) -> u64 {
        self.inner.started.elapsed().as_millis() as u64
    }

    /// The current removal epoch.
    fn epoch(&self) -> u64 {
        self.inner.epoch.load(Ordering::SeqCst)
    }

    /// Announce a removal, so creations that started before it stop writing back.
    ///
    /// # Why this is correct, on the key being removed
    ///
    /// The bump happens **before** the removal takes the key's shard lock, and
    /// every writeback reads the epoch *while holding that same lock*. The shard
    /// lock is therefore what orders the two, and there are only two cases:
    ///
    /// - the writeback holds the lock **first**. It may still read the old epoch
    ///   and reattach — and then `take_ready` acquires the lock, finds a slot
    ///   that is now ready, and removes and disposes of it. The removal wins.
    /// - the removal holds the lock **first**. The writeback acquires it
    ///   afterwards, and its read is ordered after the bump by the lock handoff,
    ///   so it sees the new epoch and fences itself.
    ///
    /// Either way the caller of `invalidate`/`evict` gets what it asked for. The
    /// counter is `SeqCst` on both sides so that this argument does not have to
    /// lean on any subtler ordering: a removal path is nowhere near hot enough
    /// for the difference to matter.
    ///
    /// # What does *not* bump
    ///
    /// Only the public removals do. The cleanup of a cancelled or panicking
    /// initializer's **empty** slot removes without bumping, on purpose: that
    /// slot's cell is what a waiter inherits, and fencing it would make the
    /// self-heal in [`reattach`](Self::reattach) impossible. The vacancy it
    /// leaves is covered instead by the disposal gate, committed under the key's
    /// shard guard — see `reattach`.
    ///
    /// # Cross-key bumps
    ///
    /// The counter is **map-wide**, not per key, so a removal on one tenant also
    /// fences detached creations for unrelated ones. That direction needs no
    /// ordering argument: over-fencing only ever disposes of a value and rebuilds
    /// it on the next request. Bumping without removing anything (`invalidate`
    /// on a tenant with nothing ready still bumps, because the bump precedes the
    /// lookup) is the same trade — a false fence costs one rebuild, a missed
    /// fence costs correctness.
    fn bump_epoch(&self) {
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
    }

    fn touch(&self, slot: &Slot<T>) {
        slot.last_used.store(self.now_millis(), Ordering::Relaxed);
    }

    fn negative_hit(&self, tenant: &TenantId, settings: &TenantedSettings) -> bool {
        let Some(ttl) = settings.negative_ttl else {
            return false;
        };
        let Some(entry) = self.inner.negative.get(tenant) else {
            return false;
        };
        let recorded = *entry.value();
        drop(entry);
        if self.now_millis().saturating_sub(recorded) < ttl.as_millis() as u64 {
            true
        } else {
            self.inner.negative.remove(tenant);
            false
        }
    }

    /// Bring the negative cache back within `max_negative`, keeping the entry
    /// [`remember_negative_owned`](Self::remember_negative_owned) just wrote.
    ///
    /// Insert first, then bound: the entry a request just learned about is the
    /// one worth keeping, and reading `len()` *before* inserting is what let
    /// concurrent unknowns all see room and overshoot the cap together. The
    /// bound is restored by purging expired entries and, if that is not enough,
    /// dropping arbitrary other entries — the negative cache is a
    /// hammer-the-directory guard, not an LRU, so which entries go is not
    /// load-bearing. Concurrent callers can push it over the bound for a moment;
    /// each of them trims, so it never *stays* over.
    fn bound_negative(&self, tenant: &TenantId, settings: &TenantedSettings) {
        if self.inner.negative.len() <= settings.max_negative {
            return;
        }
        self.purge_negative(settings);

        // Bounded: every pass removes one entry, and there are only ever
        // `len()` of them to remove.
        let mut budget = self.inner.negative.len();
        while budget > 0 && self.inner.negative.len() > settings.max_negative {
            budget -= 1;
            // The iterator's shard guard is released at the end of this
            // statement — before `remove` asks for the same shard.
            let victim = self
                .inner
                .negative
                .iter()
                .map(|entry| entry.key().clone())
                .find(|candidate| candidate != tenant);
            match victim {
                Some(victim) => {
                    self.inner.negative.remove(&victim);
                }
                // Only the entry just inserted is left: the cap is 1 (or a
                // racing purge emptied the map). Keeping it is the point.
                None => break,
            }
        }
    }

    fn purge_negative(&self, settings: &TenantedSettings) -> usize {
        let Some(ttl) = settings.negative_ttl else {
            let purged = self.inner.negative.len();
            self.inner.negative.clear();
            return purged;
        };
        let now = self.now_millis();
        let ttl = ttl.as_millis() as u64;
        let before = self.inner.negative.len();
        self.inner
            .negative
            .retain(|_, recorded| now.saturating_sub(*recorded) < ttl);
        // `saturating_sub`: concurrent unknowns insert while `retain` walks the
        // shards, so the map can be *bigger* afterwards. The count is a report,
        // not a bound — an underflow here must not take the request down.
        before.saturating_sub(self.inner.negative.len())
    }

    fn idle_since(&self, cutoff: u64) -> Vec<TenantId> {
        self.inner
            .slots
            .iter()
            .filter(|entry| {
                entry.value().is_ready()
                    && entry.value().last_used.load(Ordering::Relaxed) <= cutoff
            })
            .map(|entry| entry.key().clone())
            .collect()
    }

    async fn trim_to_max_active(&self, settings: &TenantedSettings) -> usize {
        let mut evicted = 0;
        for tenant in self.lru_victims(settings.max_active) {
            if self.evict(&tenant).await {
                evicted += 1;
                self.inner.counters.evicted_lru.fetch_add(1, Ordering::Relaxed);
            }
        }
        evicted
    }

    /// The least-recently-used ready tenants above `max_active`.
    fn lru_victims(&self, max_active: usize) -> Vec<TenantId> {
        let mut ready: Vec<(u64, TenantId)> = self
            .inner
            .slots
            .iter()
            .filter(|entry| entry.value().is_ready())
            .map(|entry| {
                (
                    entry.value().last_used.load(Ordering::Relaxed),
                    entry.key().clone(),
                )
            })
            .collect();
        if ready.len() <= max_active {
            return Vec::new();
        }
        ready.sort_by_key(|(last_used, _)| *last_used);
        let excess = ready.len() - max_active;
        ready.into_iter().take(excess).map(|(_, id)| id).collect()
    }

    /// Keep the cap even with no sweeper running: one background trim at a
    /// time, and only when the map is actually over its limit.
    ///
    /// The trim **loops**, and clears its flag before re-checking. Completions
    /// that arrive while a trim is running see `trimming = true` and return
    /// without scheduling anything, so a trim that snapshotted too few ready
    /// slots would otherwise finish and leave the map over the cap until the
    /// periodic sweep. Clearing the flag *first* and only then re-reading the
    /// map closes that handoff window: whatever the completion did or did not
    /// schedule, one of the two sides sees the excess.
    ///
    /// The re-check counts **ready** slots, not `slots.len()`, and it runs
    /// unconditionally — including after a round that evicted nothing, which is
    /// exactly the case the handoff race lives in (the last pass saw no ready
    /// excess, then a creation completed while the flag was still up). A pass
    /// over a ready excess always evicts at least one slot, so re-taking the
    /// flag there terminates.
    ///
    /// Residual, deliberately left to the periodic sweep and to the completing
    /// creations themselves: `slots.len()` can stay over the cap while
    /// `ready_count()` does not, because every excess slot is still being
    /// created and nothing can evict a creation in flight.
    fn enforce_max_active(&self, settings: &TenantedSettings) {
        if self.inner.slots.len() <= settings.max_active {
            return;
        }
        if self
            .inner
            .trimming
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let this = self.clone();
        let settings = *settings;
        if !spawn_detached(async move {
            loop {
                // One round: trim until a pass finds nothing left to evict.
                while this.trim_to_max_active(&settings).await > 0 {}
                // Clear, *then* re-read: a completion that declined to schedule
                // while this round ran is picked up here, whether or not the
                // round itself evicted anything.
                this.inner.trimming.store(false, Ordering::Release);
                if this.ready_count() <= settings.max_active {
                    break;
                }
                // Let whoever made those slots ready make progress before
                // another round: a pass that evicts nothing never awaits.
                tokio::task::yield_now().await;
                if this
                    .inner
                    .trimming
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    // Someone else took the flag between the store and here:
                    // they own the excess now.
                    break;
                }
            }
        }) {
            self.inner.trimming.store(false, Ordering::Release);
        }
    }
}

/// Outcome of a failed `create`, kept internal so the `OnceCell` initializer can
/// distinguish "unknown tenant" (which may still fall back) from a real error.
enum CreateFailure {
    Failed(TenantError),
    Unknown,
}

/// One unit of work that keeps a value alive *outside* the map, counted in
/// [`Inner::in_flight`] for as long as the guard exists.
///
/// Two things produce one, and between them they cover every way a live value
/// can be unreachable from the map while still needing to be closed:
///
/// - **A committed disposal.** [`Tenanted::commit_dispose`] mints one the
///   instant its CAS wins — *inside* the shard-lock critical section that took
///   the gate, so there is no window where the gate is committed but the work is
///   uncounted. The owner drops it when the `dispose` await returns.
/// - **A resolve holding a slot.** [`Tenanted::resolve`] mints one before it
///   touches the map and drops it once the slot is classified. This is the half
///   a disposal counter alone cannot cover: an initializer that was cancelled
///   detaches its *empty* slot, a waiter inherits the cell and fills it, and for
///   the whole stretch between that and its `reattach` there is a live value in
///   nobody's map and no gate committed anywhere.
///
/// `Drop` is the only way to discharge it, so a panicking `dispose`, a dropped
/// future and a failed spawn all decrement. Dropping one without doing the work
/// is therefore safe (it degrades to "the value is not closed", the pre-existing
/// no-runtime behaviour) — it can never wedge [`Tenanted::drain`].
struct Pending<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Pending<T> {
    fn new(inner: &Arc<Inner<T>>) -> Self {
        inner.in_flight.fetch_add(1, Ordering::SeqCst);
        Self {
            inner: Arc::clone(inner),
        }
    }
}

impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        if self.inner.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner.settled.notify_waiters();
        }
    }
}

/// The obligation to await one committed disposal — a [`Pending`] that a
/// specific caller won and must discharge.
type DisposalDebt<T> = Pending<T>;

/// A slot taken out of the map, and the disposal gate that was committed in the
/// same critical section as its removal.
///
/// `debt` is `None` only when someone else had already committed the gate — see
/// [`Tenanted::take_ready`]. The remover then removes and stands down: it cannot
/// await a disposal it does not own.
struct Removed<T> {
    slot: Arc<Slot<T>>,
    debt: Option<DisposalDebt<T>>,
}

/// Who owns a tenant's key once a creation completes.
enum SlotOwnership<T> {
    /// The map still holds this slot — the ordinary case, nothing to heal.
    Kept,
    /// The slot had been detached (a cancelled or failed earlier attempt on the
    /// same cell) and was put back: the value is the map's again.
    Restored,
    /// A concurrent resolve recreated the key with a different slot, or a
    /// removal fenced this attempt off. This value is not the map's and has to
    /// be disposed of.
    ///
    /// The slot's one-shot gate is **already committed** when this is returned —
    /// [`Tenanted::reattach`] takes it under the key's shard guard, which is what
    /// keeps a later participant from restoring a dying value. `debt` is that
    /// CAS's verdict: `Some` means this caller won it and must reach
    /// `run_committed_dispose`; `None` means someone else owns the await and this
    /// caller must **not** spawn one.
    Orphaned { debt: Option<DisposalDebt<T>> },
}

/// Removes an empty slot if the initialization it guards never returns.
///
/// Armed **inside** the `OnceCell` initializer, so only the task actually
/// running a creation carries it — a waiter parked on the cell has no guard and
/// its cancellation leaves the map alone. The `Ok` and `Err` paths of `resolve`
/// disarm it and own the map surgery themselves; what is left for the guard is
/// the two paths that return through neither — a panic inside
/// `TenantSource::create`, and the initializing future being dropped mid-create
/// (a client disconnect, a `tokio::time::timeout` around the handler). Either
/// would leave an empty slot mapped with no waiter to retry it, which a hostile
/// tenant id could farm.
///
/// The slot it detaches is not lost: a waiter that inherits the cell and
/// succeeds reattaches it (see [`Tenanted::reattach`]).
struct EmptySlotGuard<'a, T> {
    inner: &'a Inner<T>,
    tenant: &'a TenantId,
    slot: &'a Arc<Slot<T>>,
    armed: bool,
}

impl<T> Drop for EmptySlotGuard<'_, T> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Both conditions under the shard lock: `ptr_eq` never removes the
        // replacement slot a retry installed, `is_ready` never detaches a value
        // that landed in this very slot meanwhile (it would never be disposed
        // of).
        self.inner.slots.remove_if(self.tenant, |_, current| {
            Arc::ptr_eq(current, self.slot) && !current.is_ready()
        });
    }
}

/// What a request gets once [`Tenanted::drain`] has latched: a retryable 503,
/// the same class as "the tenant's resource could not be built right now".
fn draining_error(tenant: &TenantId) -> TenantError {
    TenantError::unavailable(
        tenant.clone(),
        "the per-tenant resource map is draining (shutdown)".into(),
    )
}

/// Classify a `create` failure.
///
/// A cascading source reaches its dependencies with `ctx.get::<U>()?`, which
/// boxes a [`TenantError`]. Re-wrapping that as `Unavailable` would turn a
/// missing `PerTenant` plugin or a dependency cycle (500-class wiring bugs) into
/// a retryable 503 and bury the chain one `source()` hop deeper, so a boxed
/// `TenantError` keeps its own classification. Every other cause is a genuine
/// provisioning failure for *this* tenant.
fn classify(tenant: &TenantId, cause: BoxError) -> TenantError {
    match cause.downcast::<TenantError>() {
        Ok(inner) => *inner,
        Err(cause) => TenantError::unavailable(tenant.clone(), cause),
    }
}

/// Spawn a detached task, reporting whether a runtime was available.
fn spawn_detached<F>(future: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_err() {
        return false;
    }
    // Dropping the handle detaches the task, which is the point: the caller is
    // a synchronous path (`Drop`, `invalidate`) that cannot await disposal.
    drop(r2e_core::rt::spawn(future));
    true
}

/// The background sweeper.
///
/// Wired by the [`PerTenant`](crate::PerTenant) plugin — the same shape as
/// `DbPool`'s reaper: one task per map, driven by the app's shutdown token, and
/// draining every tenant's resource when that token is cancelled.
impl<T> r2e_core::service::ServiceComponent for Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    type Deps = TCons<Self, TNil>;

    fn from_context(ctx: &BeanContext) -> Self {
        ctx.get::<Self>()
    }

    async fn start(self, shutdown: CancellationToken) {
        let interval = self.settings().sweep_interval();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    self.drain().await;
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    let report = self.sweep().await;
                    if !report.is_empty() {
                        tracing::debug!(
                            resource = std::any::type_name::<T>(),
                            idle_evicted = report.idle_evicted,
                            lru_evicted = report.lru_evicted,
                            negative_purged = report.negative_purged,
                            "swept per-tenant resources"
                        );
                    }
                }
            }
        }
    }
}

impl<T> std::fmt::Debug for Tenanted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tenanted")
            .field("resource", &std::any::type_name::<T>())
            .field("slots", &self.inner.slots.len())
            .finish()
    }
}
