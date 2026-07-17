//! Ordered-test showcase (`@Order`-style): `#[r2e::test(order = n, group = "…")]`
//! runs tagged tests sequentially in ascending order within this binary, while
//! untagged tests keep running in parallel.
//!
//! Each test records its passage in a shared sequence and asserts on exactly
//! what must have run before it — under the default parallel harness, these
//! assertions only hold because of the ordering barrier.

use std::sync::Mutex;

use example_app::ExampleApp;
use r2e_test::TestApp;

static APP_SEQ: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static PLAIN_SEQ: Mutex<Vec<u32>> = Mutex::new(Vec::new());

// ── Default group, app-boot path: the barrier covers the boot ───────────

#[r2e::test(app = ExampleApp, order = 1)]
async fn step_one_runs_first(app: TestApp) {
    app.get("/health").send().await.assert_ok();
    let mut seq = APP_SEQ.lock().unwrap();
    assert_eq!(*seq, Vec::<u32>::new());
    seq.push(1);
}

#[r2e::test(app = ExampleApp, order = 2)]
async fn step_two_sees_step_one(app: TestApp) {
    app.get("/health").send().await.assert_ok();
    let mut seq = APP_SEQ.lock().unwrap();
    assert_eq!(*seq, vec![1]);
    seq.push(2);
}

#[r2e::test(app = ExampleApp, order = 3)]
async fn step_three_sees_both(_app: TestApp) {
    let mut seq = APP_SEQ.lock().unwrap();
    assert_eq!(*seq, vec![1, 2]);
    seq.push(3);
}

// ── Named group, plain path (no app), non-contiguous orders ─────────────

#[r2e::test(order = 10, group = "plain")]
async fn plain_step_ten() {
    let mut seq = PLAIN_SEQ.lock().unwrap();
    assert_eq!(*seq, Vec::<u32>::new());
    seq.push(10);
}

#[r2e::test(order = 30, group = "plain")]
async fn plain_step_thirty() {
    let mut seq = PLAIN_SEQ.lock().unwrap();
    assert_eq!(*seq, vec![10]);
    seq.push(30);
}

// ── Untagged: unaffected, stays parallel ────────────────────────────────

#[r2e::test(app = ExampleApp)]
async fn unordered_test_still_runs(app: TestApp) {
    app.get("/health").send().await.assert_ok();
}
