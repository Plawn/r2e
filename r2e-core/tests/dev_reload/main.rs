//! Dev-reload (Subsecond hot-patch) partial rebuild, live config, and cycle
//! rollback.
//!
//! **Why its own target.** The hot-patch loop is driven by process-global,
//! one-way state: `mark_hot_reload_loop()` arms the dev caches for the rest of
//! the process, and once armed, the first served app also sets
//! `LIFECYCLE_INITIALIZED` — after which every later `run()` in that process
//! skips consumers, serve hooks and startup hooks (that is the point: a hot
//! patch must not re-run them). Ordinary serving tests sharing the binary
//! would therefore start losing their `spawn_service` tasks and startup hooks
//! as soon as one dev test had run, in a way no lock can prevent (test threads
//! run in parallel, and the flag is deliberately never disarmed in
//! production). Keeping these tests in their own process is the only thing
//! that actually separates the two worlds — see `serial.rs` for the
//! serialization that is still needed *within* this target.

#[cfg(feature = "dev-reload")]
mod config;
#[cfg(feature = "dev-reload")]
mod cycles;
#[cfg(feature = "dev-reload")]
mod rollback;
#[cfg(feature = "dev-reload")]
mod serial;
#[cfg(feature = "dev-reload")]
mod serve_hooks;
