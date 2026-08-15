//! The plugin system: what a plugin contributes to the bean graph
//! (`Provided`), what its `build` consumes (`Deps`, typed `Config`), the
//! effect surface it drives, its lifecycle, and the setup escape hatch.

#[path = "../support/mod.rs"]
mod support;

mod config;
mod deferred;
mod deps;
mod enabled;
mod fixtures;
mod late;
mod lifecycle;
mod provisions;
mod setup;
