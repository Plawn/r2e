//! `AppBuilder`: the type-level provision list, the HList state it
//! materializes, pinned overrides, the prepared/served forms, and the `App`
//! trait entry point.

#[path = "../support/mod.rs"]
mod support;

mod app;
mod hlist;
mod overrides;
mod prepared;
mod state_wiring;
mod type_list;
