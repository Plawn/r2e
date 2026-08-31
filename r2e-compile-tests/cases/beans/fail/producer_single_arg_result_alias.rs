//! A one-argument `Result` alias (`anyhow::Result<T>`, `std::io::Result<T>`)
//! hides the error type from the macro, which matches tokens. It used to fall
//! through to the infallible arm, registering the bean under the *Result* type
//! with `Error = Infallible`. Spell `Result<T, E>` out instead.

use r2e::prelude::*;

#[derive(Clone)]
pub struct Pool;

#[producer]
fn create_pool() -> std::io::Result<Pool> {
    Ok(Pool)
}

fn main() {}
