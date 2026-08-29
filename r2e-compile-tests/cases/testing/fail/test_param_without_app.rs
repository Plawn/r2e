//! The standard `#[r2e::test]` path rebuilds the function as `fn name()`, so a
//! parameter would be silently discarded. Only `app = ...` can bind one
//! (task #985).
#[r2e::test]
async fn seeded(seed: u64) {
    assert_eq!(seed, 0);
}

fn main() {}
