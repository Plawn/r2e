//! The measuring apparatus itself.
//!
//! Every other test in this target is only as trustworthy as the counter, and
//! its one non-obvious rule is how a `realloc` is charged: the full `new_size`,
//! as one event — that is the request the allocator receives, and it is what a
//! block that doubles its way up really costs. Charging the growth delta
//! instead would let a buffer that grows 1 KiB -> 2 KiB -> 4 KiB report 3 KiB
//! for 7 KiB of requests and slip under the byte canary.

use crate::counter::measure;

/// A plain allocation is one event of exactly the requested size.
#[test]
fn alloc_records_one_event_of_the_requested_size() {
    let (v, cost) = measure(|| Vec::<u8>::with_capacity(4096));
    assert_eq!(v.capacity(), 4096);
    assert_eq!(cost.count, 1, "one allocation event, got {cost}");
    assert_eq!(cost.bytes, 4096, "the requested size, got {cost}");
}

/// A grow is charged the whole new block, not the delta.
#[test]
fn realloc_growth_is_charged_the_full_new_size() {
    let mut v: Vec<u8> = Vec::with_capacity(1024);
    v.resize(1024, 0);

    let ((), cost) = measure(|| v.reserve_exact(1024));

    assert_eq!(v.capacity(), 2048);
    assert_eq!(cost.count, 1, "one realloc event, got {cost}");
    assert_eq!(
        cost.bytes, 2048,
        "a 1 KiB -> 2 KiB grow is a 2 KiB request, got {cost}",
    );
}

/// A shrink is a request too: same rule, so the accounting has no direction.
#[test]
fn realloc_shrink_is_charged_the_full_new_size() {
    let mut v: Vec<u8> = Vec::with_capacity(2048);
    v.resize(1024, 0);

    let ((), cost) = measure(|| v.shrink_to_fit());

    assert_eq!(v.capacity(), 1024);
    assert_eq!(cost.count, 1, "one realloc event, got {cost}");
    assert_eq!(cost.bytes, 1024, "the new size, got {cost}");
}

/// Repeated doubling must accumulate every request, which is the property the
/// absolute canary in `layers::composed_stack_budget` leans on.
#[test]
fn repeated_growth_accumulates_every_request() {
    let mut v: Vec<u8> = Vec::with_capacity(1024);
    v.resize(1024, 0);

    let ((), cost) = measure(|| {
        v.reserve_exact(1024); // -> 2048
        v.resize(2048, 0);
        v.reserve_exact(2048); // -> 4096
    });

    assert_eq!(v.capacity(), 4096);
    assert_eq!(cost.count, 2, "two realloc events, got {cost}");
    assert_eq!(cost.bytes, 2048 + 4096, "both full requests, got {cost}");
}

/// `dealloc` is not an allocation: freeing must not move either counter, or
/// every measured region would report the churn of the region before it.
#[test]
fn dealloc_is_not_counted() {
    let v: Vec<u8> = Vec::with_capacity(4096);
    let ((), cost) = measure(|| drop(v));
    assert_eq!(cost.count, 0, "drop must not count, got {cost}");
    assert_eq!(cost.bytes, 0, "drop must not count, got {cost}");
}

/// The counters are thread-local: work done on another thread must not land in
/// this thread's measurement. `cargo test` runs this binary's tests in
/// parallel, so without this property every assertion here would be a race.
#[test]
fn counters_do_not_cross_threads() {
    let ((), cost) = measure(|| {
        std::thread::spawn(|| {
            // Big enough that a shared counter could not miss it.
            let v: Vec<u8> = Vec::with_capacity(1 << 20);
            std::hint::black_box(&v);
        })
        .join()
        .expect("worker thread");
    });

    // `JoinHandle`/thread spawn allocates a little on this thread; the point is
    // that the megabyte allocated on the worker is not here.
    assert!(
        cost.bytes < 1 << 20,
        "another thread's allocations leaked into this measurement: {cost}",
    );
}
