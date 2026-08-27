//! Plugin system for R2E.
//!
//! Plugins are composable units of functionality installed into an
//! [`AppBuilder`] with `.plugin(..)` — **one** plugin kind, one install site.
//!
//! A [`Plugin`] is one async, fallible [`build`](Plugin::build) factory for its
//! [`Provided`](Plugin::Provided) bean tuple, executed inside `build_state()`
//! as a node of the bean graph: dependencies arrive constructed, config arrives
//! loaded. Anything it wants to do to the HTTP surface it registers as an
//! **effect** on the [`PluginBuildContext`], and effects are applied in one of
//! three stages:
//!
//! | Stage | Registered with | Applied |
//! |---|---|---|
//! | **Graph** | [`add_layer`](PluginBuildContext::add_layer), [`after_build`](PluginBuildContext::after_build), [`on_serve`](PluginBuildContext::on_serve), [`store_data`](PluginBuildContext::store_data) | inside `build_state()`, right after the graph resolves |
//! | **Routes** | [`after_routes`](PluginBuildContext::after_routes) | in `build()`, after **every** controller (app, module, plugin) is registered |
//! | **Finalize** | [`wrap_router`](PluginBuildContext::wrap_router) | in `build()`, outermost, after the whole HTTP stack is assembled |
//!
//! Within a stage, effects apply in **plugin install order** (builds run in
//! topological order — the two orders are independent). A plugin disabled via
//! `<prefix>.enabled = false` drops all three stages; its cleanup hooks
//! ([`on_shutdown`](PluginBuildContext::on_shutdown) /
//! [`on_shutdown_async`](PluginBuildContext::on_shutdown_async)) still run,
//! because `build` — and whatever it constructed — ran anyway.
//!
//! A plugin may also ship its own controllers via
//! [`Controllers`](Plugin::Controllers); they are registered by `build_state()`
//! alongside feature-module controllers and may `#[inject]` the plugin's own
//! `Provided` beans.
//!
//! The module is split by responsibility — the public surface is re-exported
//! here:
//!
//! - [`install`] — [`Plugin`], [`PluginInstall`] and its blanket impl, plugin
//!   config loading, and the [`PluginOut`] group bean.
//! - [`contexts`] — [`PluginSetupContext`], [`PluginBuildContext`],
//!   [`RoutesContext`] and the effect buckets they fill.
//! - [`graph_handle`] — [`GraphHandle`].
//! - [`deferred`] — [`DeferredAction`], [`DeferredContext`].

#[allow(unused_imports)] // referenced by intra-doc links
use crate::builder::AppBuilder;

mod contexts;
mod deferred;
mod graph_handle;
mod install;

pub use contexts::{PluginBuildContext, PluginSetupContext, RoutesContext};
pub use deferred::{AsyncShutdownHook, DeferredAction, DeferredContext};
pub use graph_handle::GraphHandle;
pub use install::{plugin_action_name, Plugin, PluginBuildError, PluginInstall, PluginOut};

#[doc(hidden)]
pub use contexts::{GraphEffect, RouterWrap, RoutesEffect};

// Crate-internal effect plumbing, re-exported to keep the `crate::plugin::*`
// paths the module split inherited.
#[allow(unused_imports)]
pub(crate) use contexts::{BuiltEffects, EffectSet, EffectsSlot};
pub(crate) use install::{load_plugin_config_from, plugin_config_enabled};
