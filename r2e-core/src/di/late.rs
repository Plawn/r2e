//! [`Late<T>`] — a shareable write-once cell, filled after the value's
//! consumers were already handed out.
//!
//! This is an **escape hatch**, not the default pattern. Plugins no longer
//! need it for their own beans: [`PreStatePlugin::build`](crate::PreStatePlugin::build)
//! runs inside `build_state()` with resolved dependencies, so plugin beans are
//! constructed whole. The framework's remaining first-party use is
//! [`GraphHandle`](crate::plugin::GraphHandle), which wraps a
//! `Late<Arc<BeanContext>>` filled right after graph resolution.
//!
//! Reach for a bare `Late<T>` only when a value genuinely cannot exist at
//! construction time — e.g. a handle produced while serving, or a cycle you
//! break deliberately — and fill it from a serve hook or deferred action.

use std::sync::{Arc, OnceLock};

/// A shareable write-once cell: created empty, filled once later, readable by
/// every clone.
///
/// Cloning shares the storage (the inner `Arc` is cloned, not the contents) —
/// a fill through any one handle is visible to every clone already handed
/// out.
///
/// The first fill wins; later fills are rejected. See the
/// [module docs](self) for when to reach for this.
pub struct Late<T> {
    slot: Arc<OnceLock<T>>,
}

impl<T> Late<T> {
    /// Create an empty cell.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slot: Arc::new(OnceLock::new()),
        }
    }

    /// Fill the cell. The first fill wins: returns `Err(value)` if it was
    /// already filled, leaving the existing value in place.
    pub fn fill(&self, value: T) -> Result<(), T> {
        self.slot.set(value)
    }

    /// The value, or `None` if the cell has not been filled yet.
    #[must_use]
    pub fn get(&self) -> Option<&T> {
        self.slot.get()
    }

    /// The value; panics if the cell has not been filled yet.
    ///
    /// `what` names the value in the panic message. Use [`get`](Self::get)
    /// for the non-panicking form (e.g. to surface a domain error instead).
    ///
    /// # Panics
    ///
    /// Panics when the cell has not been filled — the value was read before
    /// whatever fills it (a serve hook, a deferred action) ran.
    pub fn expect(&self, what: &str) -> &T {
        self.slot.get().unwrap_or_else(|| {
            panic!(
                "Late<{ty}>: `{what}` read before it was filled",
                ty = std::any::type_name::<T>()
            )
        })
    }
}

impl<T> Default for Late<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Sharing is the whole point: cloning shares the inner `Arc` so every clone
/// observes the same fill.
impl<T> Clone for Late<T> {
    fn clone(&self) -> Self {
        Self {
            slot: Arc::clone(&self.slot),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Late<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.slot.get() {
            Some(v) => f.debug_tuple("Late").field(v).finish(),
            None => f.write_str("Late(<unfilled>)"),
        }
    }
}
