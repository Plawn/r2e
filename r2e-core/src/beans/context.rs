#[allow(unused_imports)] // referenced by intra-doc links
use super::BeanRegistry;
use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

// ── BeanContext ─────────────────────────────────────────────────────────────

/// Read-only container holding all resolved bean instances.
///
/// Produced by [`BeanRegistry::resolve`]. Each entry is keyed by [`TypeId`].
///
/// Internally uses a two-layer design: a shared `Arc` base (which lazy bean
/// factories can cheaply snapshot) plus an overlay for newly added entries.
/// This avoids `Arc::try_unwrap` failures when lazy factories hold snapshots.
pub struct BeanContext {
    /// Shared base entries. May be referenced by lazy bean factories.
    pub(super) base: Arc<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    /// Overlay: entries added after the base was created. Checked first by `get()`.
    pub(super) overlay: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// Lazy bean slots: initialized on first `get::<T>()`.
    /// Shared via `Arc` so clones (used by lazy factory snapshots) see
    /// already-resolved values from the same `OnceLock` instances.
    lazy_slots: Arc<RwLock<HashMap<TypeId, Arc<dyn crate::lazy::LazyResolve>>>>,
    /// Pre-destroy disposal hooks, built during [`resolve`](BeanRegistry::resolve)
    /// from the fully resolved graph. Drained by the builder into the async
    /// shutdown phase. Not carried across [`Clone`] (lazy factory snapshots must
    /// not re-run disposal). Behind a `Mutex` so `BeanContext` stays `Sync`
    /// despite the `FnOnce` hooks (which are `Send` but not `Sync`).
    pub(super) disposers: std::sync::Mutex<Vec<crate::plugin::AsyncShutdownHook>>,
}

impl Clone for BeanContext {
    fn clone(&self) -> Self {
        Self {
            base: Arc::clone(&self.base),
            // Lazy snapshots don't need the overlay — they only depend on
            // beans that were already constructed (i.e., in the base).
            // But to keep Clone simple, we share the base and start a
            // fresh overlay. This is only used by lazy factories.
            overlay: HashMap::new(),
            // Share the same lazy slots so all clones see resolved values.
            lazy_slots: Arc::clone(&self.lazy_slots),
            // Disposal hooks are owned by the primary context only.
            disposers: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl fmt::Debug for BeanContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lazy_count = self.lazy_slots.read().map(|m| m.len()).unwrap_or(0);
        f.debug_struct("BeanContext")
            .field("base_count", &self.base.len())
            .field("overlay_count", &self.overlay.len())
            .field("lazy_count", &lazy_count)
            .finish()
    }
}

impl BeanContext {
    /// Create an empty context (no beans).
    ///
    /// Used as the placeholder before graph resolution and for the
    /// [`with_state`](crate::AppBuilder::with_state) path, which bypasses the
    /// bean graph entirely.
    pub fn empty() -> Self {
        Self::new(HashMap::new())
    }

    /// Create a new BeanContext wrapping the given entries as the shared base.
    pub(super) fn new(entries: HashMap<TypeId, Box<dyn Any + Send + Sync>>) -> Self {
        Self {
            base: Arc::new(entries),
            overlay: HashMap::new(),
            lazy_slots: Arc::new(RwLock::new(HashMap::new())),
            disposers: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Drain the pre-destroy disposal hooks built during resolution.
    ///
    /// Called once by the builder to move the disposers into the async
    /// shutdown phase. Returns an empty vec on any context that never carried
    /// disposers (e.g. a lazy-factory snapshot clone or the `with_state` path).
    #[doc(hidden)]
    pub fn take_disposers(&mut self) -> Vec<crate::plugin::AsyncShutdownHook> {
        std::mem::take(&mut *self.disposers.lock().expect("disposers lock poisoned"))
    }

    /// Attach lazy bean slots to this context.
    pub(super) fn with_lazy_slots(
        mut self,
        slots: Arc<RwLock<HashMap<TypeId, Arc<dyn crate::lazy::LazyResolve>>>>,
    ) -> Self {
        self.lazy_slots = slots;
        self
    }

    /// Insert a new entry, creating a new context that shares the same base.
    ///
    /// If the base `Arc` has no other references, the new entry is merged
    /// into the base directly (zero overhead). Otherwise the entry goes
    /// into the overlay (which is checked first by `get()`).
    pub(super) fn with_new_entry(mut self, type_id: TypeId, value: Box<dyn Any + Send + Sync>) -> Self {
        // Fast path: if we're the sole owner of the base, merge everything
        // into a single HashMap for the next iteration.
        if let Some(base) = Arc::get_mut(&mut self.base) {
            // Drain overlay into base first
            for (k, v) in self.overlay.drain() {
                base.insert(k, v);
            }
            base.insert(type_id, value);
        } else {
            // A lazy factory holds a snapshot of the base. New entries
            // go into the overlay.
            self.overlay.insert(type_id, value);
        }
        self
    }

    /// Retrieve a bean by type, cloning it out of the context.
    ///
    /// Checks the overlay first, then the shared base. If the bean is not
    /// found eagerly, checks the lazy slots and constructs it on first access.
    ///
    /// # Panics
    ///
    /// Panics if the requested type was not registered or provided.
    pub fn get<T: Clone + 'static>(&self) -> T {
        self.try_get::<T>()
            .unwrap_or_else(|| panic!("Bean of type `{}` not found in context", type_name::<T>()))
    }

    /// Try to retrieve an **eagerly constructed** bean by type, without
    /// touching the lazy slots. Used by the dev-reload partial rebuild to
    /// clone unchanged instances out of the previous cycle's context —
    /// a lazy bean must never be force-resolved just to be carried over
    /// (its slot `Arc` is reused instead).
    pub(super) fn try_get_eager<T: Clone + 'static>(&self) -> Option<T> {
        let tid = TypeId::of::<T>();
        self.overlay
            .get(&tid)
            .or_else(|| self.base.get(&tid))
            .and_then(|v| v.downcast_ref::<T>())
            .cloned()
    }

    /// Look up a lazy slot by `TypeId`. Used by the dev-reload partial
    /// rebuild to carry an unchanged lazy bean's slot (and any
    /// already-resolved value inside it) into the next cycle's context.
    pub(super) fn lazy_slot(&self, tid: TypeId) -> Option<Arc<dyn crate::lazy::LazyResolve>> {
        self.lazy_slots
            .read()
            .ok()
            .and_then(|slots| slots.get(&tid).map(Arc::clone))
    }

    /// Try to retrieve a bean by type, returning `None` if absent.
    pub fn try_get<T: Clone + 'static>(&self) -> Option<T> {
        let tid = TypeId::of::<T>();
        // Fast path: eagerly-constructed bean (overlay → base)
        if let Some(val) = self
            .overlay
            .get(&tid)
            .or_else(|| self.base.get(&tid))
            .and_then(|v| v.downcast_ref::<T>())
        {
            return Some(val.clone());
        }
        // Lazy path: construct on first access
        let slot = self
            .lazy_slots
            .read()
            .ok()
            .and_then(|slots| slots.get(&tid).map(Arc::clone))?;
        let resolved = slot.resolve();
        resolved.downcast_ref::<T>().cloned()
    }
}
