//! [`Late<T>`] — a shareable write-once cell for beans finished after
//! `build_state()`.
//!
//! A [`PreStatePlugin`](crate::PreStatePlugin) installs *before* the bean
//! graph is built, so a provided bean that needs another bean cannot be
//! constructed at install time. The pattern is to provide a **shell** holding
//! a `Late<T>` and fill it post-state, when the plugin's `Deps` are resolved:
//!
//! - **sync fill** — call [`Late::fill`] from
//!   [`configure`](crate::PreStatePlugin::configure), which receives the
//!   resolved deps by value;
//! - **async fill** — register the fill as a bean post-construct via
//!   [`PluginInstallContext::run_post_construct`](crate::PluginInstallContext::run_post_construct);
//!   it runs (and is awaited) inside `build_state()`. This is how the
//!   `OpenFga` plugin boots its gRPC backend.
//!
//! Either way the cell is filled before `build_state()` returns, so
//! application code reading it after boot never observes it empty.

use std::sync::{Arc, OnceLock};

/// A shareable write-once cell: empty at plugin install, filled once
/// post-state, readable by every clone.
///
/// Cloning shares the storage (the inner `Arc` is cloned, not the contents) —
/// the same contract as beans in the graph, which are cloned by value
/// everywhere *before* any post-state fill runs. A fill through any one
/// handle is visible to every clone already handed out.
///
/// The first fill wins; later fills are rejected. See the
/// [module docs](self) for the install/fill lifecycle.
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
    ///
    /// After `build_state()` a plugin-filled cell is always `Some` — `None`
    /// signals a pre-boot read (or a plugin disabled via `<prefix>.enabled`).
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
    /// Panics on a pre-boot read — the cell is filled during `build_state()`
    /// (plugin `configure` or an awaited post-construct), so a panic here
    /// means the value escaped the builder before boot finished, or its
    /// plugin was disabled.
    pub fn expect(&self, what: &str) -> &T {
        self.slot.get().unwrap_or_else(|| {
            panic!(
                "Late<{ty}>: `{what}` read before it was filled — the cell is filled during \
                 `build_state()` (plugin `configure` or an awaited post-construct). Reading \
                 earlier than that, or with the owning plugin disabled, finds it empty.",
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
