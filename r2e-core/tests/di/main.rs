//! Dependency injection: the bean graph and everything that shapes it —
//! resolution order, async beans, producers, optional/lazy beans, defaults vs.
//! alternatives, pinned overrides, lifecycle hooks, and feature modules.

#[path = "../support/mod.rs"]
mod support;

mod async_beans;
mod context;
mod defaults;
#[cfg(feature = "dev-reload")]
mod fingerprint;
mod fixtures;
mod graph;
mod lazy_bean;
mod lazy_cell;
mod lifecycle;
mod module;
mod option_config;
mod option_type;
mod optional;
mod pinned;
mod producers;
