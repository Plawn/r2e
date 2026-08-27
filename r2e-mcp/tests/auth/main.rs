//! Auth subsystem tests: OAuth 2.1 resource-server layer, well-knowns, DCR
//! shim, discovery, scope policy and per-tool authorization.

#[path = "../support/mod.rs"]
mod support;

mod fixtures;

mod backends;
mod config;
mod discovery;
mod layer;
mod scopes;
mod shim;
mod tools;
mod wellknown;

#[cfg(feature = "testing")]
mod pin;
