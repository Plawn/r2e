//! Tests for the ordered-test barrier ([`r2e_test::ordering`]).
//!
//! [`turn`] is driven directly here (the proc macro that normally emits it
//! cannot be exercised from inside `r2e-test`). Because barrier state is
//! process-global and test binaries run their tests in parallel, every test
//! below uses a **distinct** group name so the groups never interfere.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use r2e_test::ordering::inventory;
use r2e_test::ordering::{OrderedTestEntry, turn, turn_with_timeout};

/// Run an async block to completion on a fresh single-thread runtime, the way a
/// generated `#[r2e::test]` body would (each test builds its own runtime).
fn run<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

// ---------------------------------------------------------------------------
// 1. Out-of-order arrival still executes in ascending order.
// ---------------------------------------------------------------------------

inventory::submit! { OrderedTestEntry { group: "seq_basic", order: 1, test: "t1" } }
inventory::submit! { OrderedTestEntry { group: "seq_basic", order: 2, test: "t2" } }
inventory::submit! { OrderedTestEntry { group: "seq_basic", order: 3, test: "t3" } }

#[test]
fn ordered_arrival_is_ascending() {
    let observed: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));

    let spawn = |order: u32, startup_delay: Duration| {
        let observed = observed.clone();
        std::thread::spawn(move || {
            // Force the "wrong" arrival order: order 3 arrives first, etc.
            std::thread::sleep(startup_delay);
            run(async move {
                let _guard = turn("seq_basic", order, "t").await;
                observed.lock().unwrap().push(order);
                // Hold the turn briefly so the ordering is observable.
                tokio::time::sleep(Duration::from_millis(20)).await;
            });
        })
    };

    // Spawn 3 first, then 2, then 1, each staggered so 3 reaches `turn` first.
    let h3 = spawn(3, Duration::from_millis(0));
    let h2 = spawn(2, Duration::from_millis(30));
    let h1 = spawn(1, Duration::from_millis(60));

    h1.join().unwrap();
    h2.join().unwrap();
    h3.join().unwrap();

    assert_eq!(*observed.lock().unwrap(), vec![1, 2, 3]);
}

// ---------------------------------------------------------------------------
// 2. Non-contiguous orders (10, 20, 30) work.
// ---------------------------------------------------------------------------

inventory::submit! { OrderedTestEntry { group: "seq_gap", order: 10, test: "g10" } }
inventory::submit! { OrderedTestEntry { group: "seq_gap", order: 20, test: "g20" } }
inventory::submit! { OrderedTestEntry { group: "seq_gap", order: 30, test: "g30" } }

#[test]
fn non_contiguous_orders() {
    let observed: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));

    let spawn = |order: u32, startup_delay: Duration| {
        let observed = observed.clone();
        std::thread::spawn(move || {
            std::thread::sleep(startup_delay);
            run(async move {
                let _guard = turn("seq_gap", order, "g").await;
                observed.lock().unwrap().push(order);
                tokio::time::sleep(Duration::from_millis(20)).await;
            });
        })
    };

    let h30 = spawn(30, Duration::from_millis(0));
    let h20 = spawn(20, Duration::from_millis(30));
    let h10 = spawn(10, Duration::from_millis(60));

    h10.join().unwrap();
    h20.join().unwrap();
    h30.join().unwrap();

    assert_eq!(*observed.lock().unwrap(), vec![10, 20, 30]);
}

// ---------------------------------------------------------------------------
// 3. A panic in order 1 fails order 2 fast (poison), without hanging.
// ---------------------------------------------------------------------------

inventory::submit! { OrderedTestEntry { group: "seq_poison", order: 1, test: "p1" } }
inventory::submit! { OrderedTestEntry { group: "seq_poison", order: 2, test: "p2" } }

#[test]
fn panic_poisons_group_and_skips_successors() {
    // Order 1: acquire turn, then panic like a failing test body.
    let h1 = std::thread::spawn(|| {
        run(async {
            let _guard = turn("seq_poison", 1, "p1").await;
            panic!("boom in order 1");
        });
    });

    // Order 2: must panic fast with the poison message, not hang.
    let h2 = std::thread::spawn(|| {
        run(async {
            let _guard = turn("seq_poison", 2, "p2").await;
        });
    });

    let err1 = h1.join().expect_err("order 1 should panic");
    let msg1 = panic_message(&err1);
    assert!(msg1.contains("boom in order 1"), "unexpected order-1 panic: {msg1}");

    let err2 = h2.join().expect_err("order 2 should panic (poisoned)");
    let msg2 = panic_message(&err2);
    assert!(msg2.contains("skipped"), "order-2 panic missing 'skipped': {msg2}");
    assert!(msg2.contains("predecessor"), "order-2 panic missing 'predecessor': {msg2}");
    assert!(msg2.contains("'p1'"), "order-2 panic should name predecessor p1: {msg2}");
}

// ---------------------------------------------------------------------------
// 4. Duplicate (group, order) → both turns panic with the duplicate diagnostic.
// ---------------------------------------------------------------------------

inventory::submit! { OrderedTestEntry { group: "seq_dup", order: 1, test: "dup_a" } }
inventory::submit! { OrderedTestEntry { group: "seq_dup", order: 1, test: "dup_b" } }

#[test]
fn duplicate_registration_panics_both() {
    let h_a = std::thread::spawn(|| {
        run(async {
            let _guard = turn("seq_dup", 1, "dup_a").await;
        });
    });
    let h_b = std::thread::spawn(|| {
        run(async {
            let _guard = turn("seq_dup", 1, "dup_b").await;
        });
    });

    let ea = h_a.join().expect_err("dup_a should panic");
    let eb = h_b.join().expect_err("dup_b should panic");
    for (label, err) in [("dup_a", &ea), ("dup_b", &eb)] {
        let msg = panic_message(err);
        assert!(msg.contains("duplicate ordered test"), "{label}: {msg}");
        assert!(msg.contains("seq_dup"), "{label} missing group: {msg}");
        assert!(msg.contains("dup_a") && msg.contains("dup_b"), "{label} missing both paths: {msg}");
    }
}

// ---------------------------------------------------------------------------
// 5. Watchdog: a registered-but-never-run predecessor makes the waiter panic
//    with the "never started" diagnostic quickly (injected short timeout).
// ---------------------------------------------------------------------------

inventory::submit! { OrderedTestEntry { group: "seq_watchdog", order: 1, test: "wd1" } }
inventory::submit! { OrderedTestEntry { group: "seq_watchdog", order: 2, test: "wd2" } }

#[test]
fn watchdog_reports_never_started() {
    // Order 1 is registered but we never call turn() for it. Order 2 waits and
    // must trip the watchdog fast thanks to the injected short timeout.
    let start = std::time::Instant::now();
    let err = std::thread::spawn(|| {
        run(async {
            let _guard =
                turn_with_timeout("seq_watchdog", 2, "wd2", Duration::from_millis(200)).await;
        });
    })
    .join()
    .expect_err("order 2 should time out");

    let msg = panic_message(&err);
    assert!(msg.contains("timed out"), "watchdog message missing 'timed out': {msg}");
    assert!(msg.contains("never started"), "watchdog should flag 'never started': {msg}");
    assert!(msg.contains("wd1"), "watchdog should name the pending predecessor wd1: {msg}");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "watchdog took too long: {:?}",
        start.elapsed()
    );
}

/// Extract a readable string from a caught panic payload.
fn panic_message(err: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = err.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = err.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}
