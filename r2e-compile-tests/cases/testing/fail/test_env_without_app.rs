//! `env = ...` hands an already-built `App::Env` to the boot, so it only means
//! something when the test boots an app (task #988).
fn env() {}

#[r2e::test(env = env())]
async fn shares_an_environment() {}

fn main() {}
