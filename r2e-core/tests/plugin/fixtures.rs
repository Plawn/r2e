//! Fixtures shared by more than one plugin test module.

#![allow(dead_code)]

/// A plain marker bean a plugin can contribute through `Provided`.
#[derive(Clone, Debug, PartialEq)]
pub struct Alpha(pub u32);

/// A second one, to exercise multi-provision plugins.
#[derive(Clone, Debug, PartialEq)]
pub struct Beta(pub String);

/// Marker bean for plugins that only exist to drive the post-state sugar.
#[derive(Clone)]
pub struct SugarMarker;

/// Data deposited via `ctx.store_data` sugar.
pub struct StoredData(pub u32);
