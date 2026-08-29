//! A counting global allocator and the measurement helpers built on it.
//!
//! Thread-local rather than global counters: `cargo test` runs the tests of a
//! binary on several threads at once, and a process-wide `AtomicU64` would mix
//! their allocations together. The thread-locals are `const`-initialised and
//! hold a `Copy` type with no `Drop`, so reading them never allocates and never
//! re-enters the allocator (which would deadlock or recurse).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

/// `System`, plus a per-thread tally of allocation events and bytes requested.
pub struct CountingAllocator;

#[inline]
fn record(bytes: usize) {
    // `try_with`: during thread teardown the TLS block may already be gone.
    // A missed tally there is harmless — no measurement is in flight.
    let _ = COUNT.try_with(|c| c.set(c.get().wrapping_add(1)));
    let _ = BYTES.try_with(|b| b.set(b.get().wrapping_add(bytes as u64)));
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // A grow counts as one allocation event of the growth delta. Delegating
        // to `System::realloc` (rather than letting the default trait impl do
        // alloc+copy+dealloc) keeps in-place growth in place, so the harness
        // does not distort what it measures.
        record(new_size.saturating_sub(layout.size()));
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Allocation events and bytes requested over a measured region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Alloc {
    pub count: u64,
    pub bytes: u64,
}

impl Alloc {
    /// Normalise a batch measurement to a per-iteration figure.
    pub fn per(self, iterations: u64) -> Alloc {
        assert!(iterations > 0, "cannot normalise over zero iterations");
        Alloc {
            count: self.count / iterations,
            bytes: self.bytes / iterations,
        }
    }
}

impl std::fmt::Display for Alloc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} allocations / {} bytes", self.count, self.bytes)
    }
}

fn snapshot() -> Alloc {
    Alloc {
        count: COUNT.with(Cell::get),
        bytes: BYTES.with(Cell::get),
    }
}

/// Run `f` and report what it allocated on this thread.
///
/// Anything the caller does outside the closure — formatting, printing,
/// building fixtures — is excluded, so keep the closure to the code under test.
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Alloc) {
    let before = snapshot();
    let out = f();
    let after = snapshot();
    (
        out,
        Alloc {
            count: after.count - before.count,
            bytes: after.bytes - before.bytes,
        },
    )
}

/// Warm up `iterations` times, then measure `iterations` more and return the
/// per-iteration cost.
pub fn steady_state(iterations: u64, mut f: impl FnMut()) -> Alloc {
    for _ in 0..iterations {
        f();
    }
    let ((), total) = measure(|| {
        for _ in 0..iterations {
            f();
        }
    });
    total.per(iterations)
}

/// A `current_thread` runtime: every measured future is driven to completion on
/// the calling thread, so the thread-local counter sees all of its allocations
/// and none of any other test's.
pub fn runtime() -> r2e::rt::Runtime {
    r2e::rt::RuntimeBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime")
}

/// Assert that a wrapper's per-request cost did not grow with the size of its
/// immutable configuration.
///
/// `slack_*` absorbs incidental jitter (a differently-sized `format!` buffer,
/// one extra small `Vec`) while staying far below what a deep clone of the
/// large config would cost — every call site sizes its large config so the
/// regression would blow past the slack by an order of magnitude.
#[track_caller]
pub fn assert_config_size_invariant(
    what: &str,
    small: Alloc,
    large: Alloc,
    slack_count: u64,
    slack_bytes: u64,
) {
    eprintln!("[hotpath] {what}: small config = {small}; large config = {large} (per request)");
    assert!(
        large.count <= small.count + slack_count,
        "{what}: per-request allocation COUNT grows with the immutable config \
         ({} -> {}, slack {slack_count}). The config is being deep-cloned per \
         request instead of shared behind an Arc — see \
         docs/claude/hot-path-clone-audit.md.",
        small.count,
        large.count,
    );
    assert!(
        large.bytes <= small.bytes + slack_bytes,
        "{what}: per-request allocated BYTES grow with the immutable config \
         ({} -> {}, slack {slack_bytes}). The config is being deep-cloned per \
         request instead of shared behind an Arc — see \
         docs/claude/hot-path-clone-audit.md.",
        small.bytes,
        large.bytes,
    );
}
