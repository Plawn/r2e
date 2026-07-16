//! Example application library.
//!
//! The canonical source lives in `app.rs`. It is included here so integration
//! tests can boot `ExampleApp`, while dev mode compiles the same file into the
//! binary tip crate for real Subsecond hot-patching.

include!("app.rs");
