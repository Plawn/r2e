//! Integration tests for the r2e-mcp server core: the `McpServer` plugin,
//! `#[routes]` service registration and dispatch, generated schemas,
//! decorators (guards/interceptors), and serve lifecycle.

#[path = "../support/mod.rs"]
mod support;

mod fixtures;

mod access;
mod completion;
mod dispatch;
mod dynamic;
mod elicitation;
mod global;
mod identity;
mod interceptors;
mod lifecycle;
#[cfg(feature = "loopback")]
mod loopback;
#[cfg(feature = "loopback")]
mod loopback_extensions;
mod methods;
mod mount;
mod mrtr;
mod pagination;
mod plugin;
mod progress;
mod prompts;
mod protocol_version;
mod raw_error;
mod registry;
mod rejection;
mod resources;
mod schema;
mod persistence;
mod sessions;
#[cfg(feature = "testing")]
mod test_client;
mod view_filter;
mod wire_golden;
