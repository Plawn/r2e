//! example-grpc library.
//!
//! The canonical source lives in `app.rs`. It is included here so the app can
//! be booted by type, while `app_main!` compiles the same file into the binary
//! tip crate for production and real Subsecond hot-patching.

include!("app.rs");
