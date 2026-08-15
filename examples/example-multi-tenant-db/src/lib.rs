//! example-multi-tenant-db library.
//!
//! The canonical source lives in `app.rs`. It is included here so integration
//! tests can boot `MultiTenantDbApp`, while `app_main!` compiles the same file
//! into the binary tip crate.

include!("app.rs");
