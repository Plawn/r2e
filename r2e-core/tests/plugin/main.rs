//! The plugin system: what a plugin contributes to the bean graph
//! (`Provided`), what it reads back (`Deps`, `Late<T>`), the deferred
//! post-state surface it drives, its typed config, and its lifecycle.

#[path = "../support/mod.rs"]
mod support;

mod config;
mod deferred;
mod deps;
mod enabled;
mod fixtures;
mod install_context;
mod late;
mod lifecycle;
mod provisions;
