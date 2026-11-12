//! Performance budgets: deterministic regression guards for the request hot
//! path and for dependency injection.
//!
//! Wall-clock time is too noisy on shared CI runners to gate a PR on, so this
//! target counts things that do not depend on the machine: **heap
//! allocations** (through a counting global allocator) and **bean clones**.
//! Each assertion compares a measured count to a named budget constant.
//!
//! Most budgets are *relative* — an R2E controller against a hand-written
//! handler on the same application stack, a graph of 64 beans against one of
//! 32 — so a dependency upgrade that shifts the absolute numbers does not
//! trip them. The few absolute ones say so next to the constant.
//!
//! When a test here fails, the change under review made a request or the
//! graph build more expensive. Either remove the cost or, when it is the
//! price of a feature, raise the budget in the same PR and say why in the
//! commit — a budget is a decision, never a number to nudge until green.
//!
//! Its **own target**: `#[global_allocator]` is process-global. Counters are
//! thread-local and every test runs on a `current_thread` runtime, so tests
//! in this binary can still run in parallel without seeing each other.
//!
//! What this does not see: CPU cost that allocates nothing (an accidentally
//! quadratic pass over the graph, a slower extractor). The criterion benches
//! in `benches/` remain the tool to investigate those.

// The 64-bean state in `di` is a type-level list of that depth.
#![recursion_limit = "256"]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

mod di;
mod dispatch;
mod fixtures;
mod request_scope;

struct CountingAllocator;

thread_local! {
    // `const` + no destructor: safe to touch from inside the allocator, at
    // any point of a thread's life.
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

#[inline]
fn record() {
    let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
}

// SAFETY: every method forwards to `System` unchanged; the only addition is a
// thread-local counter bump that neither allocates nor unwinds.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc_zeroed(layout) }
    }

    // A growth is a trip to the allocator like any other.
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

/// Allocations made so far by the current thread.
fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}
