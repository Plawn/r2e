//! The negative cache: unknown tenants, remembered briefly and bounded.

use crate::TenantId;

use super::state::Tenanted;
use super::TenantedSettings;

impl<T> Tenanted<T>
where
    T: Clone + Send + Sync + 'static,
{
    pub(super) fn negative_hit(&self, tenant: &TenantId, settings: &TenantedSettings) -> bool {
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
    pub(super) fn bound_negative(&self, tenant: &TenantId, settings: &TenantedSettings) {
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

    pub(super) fn purge_negative(&self, settings: &TenantedSettings) -> usize {
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
}
