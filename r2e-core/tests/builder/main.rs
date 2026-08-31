//! `AppBuilder`: the type-level provision list, the HList state it
//! materializes, pinned overrides, the prepared/served forms, and the `App`
//! trait entry point.

#[path = "../support/mod.rs"]
mod support;

mod app;
mod hlist;
mod on_start_once;
mod overrides;
mod prepared;
mod provide_bundle;
mod service_start;
mod shutdown_token;
mod state_wiring;
mod type_list;
