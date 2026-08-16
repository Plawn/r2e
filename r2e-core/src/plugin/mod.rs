//! Plugin system for R2E.
//!
//! Plugins are composable units of functionality installed into an
//! [`AppBuilder`].
//!
//! # Two plugin traits
//!
//! - [`PreStatePlugin`]: For plugins that provide beans (like Scheduler).
//!   Installed with `.plugin(p)` **before** `build_state()`. A pre-state
//!   plugin is one async, fallible [`build`](PreStatePlugin::build) factory
//!   for its `Provided` tuple, executed inside `build_state()` as a node of
//!   the bean graph — dependencies arrive constructed, config arrives loaded.
//! - [`Plugin`]: For plugins that don't provide beans. Installed with
//!   `.with(p)` **after** `build_state()`, with full router access.
//!
//! The module is split by responsibility — the public surface is unchanged and
//! is re-exported here:
//!
//! - [`post_state`] — the [`Plugin`] trait.
//! - [`pre_state`] — [`PreStatePlugin`], [`RawPreStatePlugin`] and its blanket
//!   impl, plugin config loading, and the [`PluginOut`] group bean.
//! - [`contexts`] — [`PluginSetupContext`], [`PluginBuildContext`] and the
//!   effect buckets they fill.
//! - [`graph_handle`] — [`GraphHandle`].
//! - [`deferred`] — [`DeferredAction`], [`DeferredContext`].

#[allow(unused_imports)] // referenced by intra-doc links
use crate::builder::AppBuilder;

mod contexts;
mod deferred;
mod graph_handle;
mod post_state;
mod pre_state;

pub use contexts::{PluginBuildContext, PluginSetupContext};
pub use deferred::{AsyncShutdownHook, DeferredAction, DeferredContext};
pub use graph_handle::GraphHandle;
pub use post_state::Plugin;
pub use pre_state::{
    plugin_action_name, PluginBuildError, PluginOut, PreStatePlugin, RawPreStatePlugin,
};

// `BuiltEffects` / `PluginEffects` are re-exported to keep the crate-internal
// `crate::plugin::*` paths they had before the module split.
#[allow(unused_imports)]
pub(crate) use contexts::{BuiltEffects, EffectsSlot, PluginEffects};
pub(crate) use pre_state::{load_plugin_config_from, plugin_config_enabled};
