//! Configuration: `R2eConfig`, the `ConfigProperties` derive and its sections,
//! `FromConfigValue`, environment overlays, secret resolution, file loading,
//! and startup validation.

#[path = "../support/mod.rs"]
mod support;

mod core;
mod env;
mod errors;
mod live_config;
mod loader;
mod properties;
mod provider;
mod registry;
mod secrets;
mod sections;
mod validation;
mod value;
