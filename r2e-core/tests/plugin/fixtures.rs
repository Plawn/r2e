//! Fixtures shared by more than one plugin test module.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A plain marker bean a plugin can contribute through `Provided`.
#[derive(Clone, Debug, PartialEq)]
pub struct Alpha(pub u32);

/// A second one, to exercise multi-provision plugins.
#[derive(Clone, Debug, PartialEq)]
pub struct Beta(pub String);

/// A third one, for arity-3 provisions.
#[derive(Clone, Debug, PartialEq)]
pub struct Gamma(pub bool);

/// Marker bean for plugins that only exist to drive the effect-stage sugar.
#[derive(Clone)]
pub struct SugarMarker;

/// Data deposited via `ctx.store_data` sugar.
pub struct StoredData(pub u32);

/// Data deposited from `setup()` (as opposed to `build`), so a test can tell
/// the two action slots apart.
pub struct SetupData(pub u32);

/// A shared flag a plugin's `build` flips, so tests can prove whether `build`
/// ran at all (all-pinned skip) — or ran despite an empty `Provided` tuple.
#[derive(Clone, Default)]
pub struct BuildProbe(Arc<AtomicBool>);

impl BuildProbe {
    pub fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn ran(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A shared append-only log for ordering assertions across builds and effects.
#[derive(Clone, Default)]
pub struct EventLog(Arc<Mutex<Vec<&'static str>>>);

impl EventLog {
    pub fn push(&self, event: &'static str) {
        self.0.lock().unwrap().push(event);
    }
    pub fn entries(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}
