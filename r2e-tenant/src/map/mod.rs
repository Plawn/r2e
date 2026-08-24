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
//!   Residual behaviour, inherited from `rt::sync::OnceCell`: a failed
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

#[allow(unused_imports)]
use crate::source::TenantSource;
#[allow(unused_imports)]
use crate::TenantId;

mod api;
mod dispose;
mod drain;
mod epoch;
mod eviction;
mod metrics;
mod negative;
mod resolve;
mod settings;
mod state;
mod test_seams;

pub use metrics::{SweepReport, TenantStats, TenantedMetrics};
pub use settings::TenantedSettings;
pub use state::Tenanted;
