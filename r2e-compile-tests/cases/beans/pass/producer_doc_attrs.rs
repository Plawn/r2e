//! `#[producer]` forwards the annotated function's own attributes — doc
//! comments above all — onto the function it re-emits (task #985).
//!
//! `missing_docs` only fires on items that are effectively public from the
//! crate root, which is why this assertion lives in a trybuild case (the file
//! IS the crate root) rather than in an integration-test module.
//!
//! Denying it here means:
//!   * the doc comment on `make_documented` must survive the rebuild, and
//!   * the producer struct the macro synthesises (`MakeDocumented`, which
//!     inherits the function's visibility) must carry a doc of its own.
#![deny(missing_docs)]

use r2e::prelude::*;

/// The produced bean.
#[derive(Clone)]
pub struct Documented(pub &'static str);

/// Builds the [`Documented`] bean.
///
/// Without attribute forwarding the emitted function carries no `#[doc]` and
/// `missing_docs` denies it.
#[producer]
pub fn make_documented() -> Documented {
    Documented("documented")
}

/// `#[inline]` is not a lint attribute — it simply has to arrive.
#[inline]
#[producer]
pub fn inlined() -> Documented {
    Documented("inlined")
}

fn main() {
    let _ = MakeDocumented;
    let _ = Inlined;
}
