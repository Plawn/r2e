//! A single-argument alias (`anyhow::Result<Self>` and friends) is NOT a
//! fallible constructor: the macro matches tokens, not resolved types, so it
//! sees a return that is neither `Self` nor a literal `Result<Self, E>` and
//! reports that the impl has no constructor at all. Spell the error out —
//! `-> Result<Self, E>`.

use r2e::prelude::*;

type Fallible<T> = std::result::Result<T, std::io::Error>;

#[derive(Clone)]
pub struct MyService;

#[bean]
impl MyService {
    fn new() -> Fallible<Self> {
        Ok(MyService)
    }
}

fn main() {}
