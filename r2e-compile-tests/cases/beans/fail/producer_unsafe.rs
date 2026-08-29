//! `#[producer]` on an `unsafe fn` is a compile error.
//!
//! The generated `Producer::produce` is a safe method and the ONLY caller, so
//! the `unsafe` contract has nobody to discharge it: re-emitting the signature
//! verbatim is an E0133, and silently wrapping the call in `unsafe { }` would
//! sign the contract on the user's behalf. Both are worse than refusing the
//! declaration (task #985).
use r2e::prelude::*;

#[derive(Clone)]
pub struct Raw(pub u8);

#[producer]
unsafe fn make_raw() -> Raw {
    Raw(1)
}

#[producer]
async unsafe fn make_raw_async() -> Raw {
    Raw(2)
}

#[producer]
async unsafe fn make_raw_fallible() -> Result<Raw, std::io::Error> {
    Ok(Raw(3))
}

fn main() {}
