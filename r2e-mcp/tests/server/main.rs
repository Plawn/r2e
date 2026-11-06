//! Integration tests for the r2e-mcp server core: the `McpServer` plugin,
//! `#[routes]` service registration and dispatch, generated schemas,
//! decorators (guards/interceptors), and serve lifecycle.

#[path = "../support/mod.rs"]
mod support;

mod fixtures;

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
mod mount;
mod pagination;
mod plugin;
mod progress;
mod prompts;
mod protocol_version;
mod registry;
mod rejection;
mod resources;
mod schema;
mod sessions;
mod wire_golden;
