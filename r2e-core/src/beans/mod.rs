//! Bean graph: the registration traits, the resolved [`BeanContext`], and the
//! [`BeanRegistry`] that validates and resolves the dependency graph.
//!
//! The module is split by responsibility — the public surface is unchanged and
//! is re-exported here:
//!
//! - [`traits`] — [`Bean`], [`AsyncBean`], [`Producer`], [`PostConstruct`],
//!   [`PreDestroy`], [`Registrable`].
//! - [`context`] — [`BeanContext`], the resolved read-only container.
//! - [`error`] — [`BeanError`].
//! - [`registry`] — the [`BeanRegistry`] struct, its registration records and
//!   the internal hook type aliases.
//! - [`registry_provide`] / [`plugin_nodes`] — the registration entry points
//!   (`provide`/`register`/`register_producer`, plugin group + projection nodes).
//! - [`graph`] — deduplication, validation and topological sorting.
//! - [`resolve`] — graph resolution and bean construction.
//! - [`reuse`] — dev-reload fingerprinting and the [`ReusePlan`].

mod context;
mod error;
mod graph;
mod plugin_nodes;
mod registry;
mod registry_provide;
mod resolve;
mod reuse;
mod traits;

pub use context::BeanContext;
pub use error::BeanError;
pub use registry::BeanRegistry;
pub use reuse::ReusePlan;
#[cfg(feature = "dev-reload")]
pub use traits::BeanFingerprints;
pub use traits::{AsyncBean, Bean, PostConstruct, PreDestroy, Producer, Registrable};

pub(crate) use registry::ServiceSourceHook;

use registry::{
    BeanRegistration, Factory, LazyBeanRegistration, PostConstructFn, RegMeta, ServiceConfigDecl,
};
use reuse::{reuse_clone_none, reuse_clone_of, ReuseCloneFn};
