//! Same rejection on `#[r2e::test]`: `start_paused` without
//! `flavor = "current_thread"` panics inside the runtime builder.

#[r2e::test(start_paused = true)]
async fn paused_without_current_thread() {}

fn main() {}
