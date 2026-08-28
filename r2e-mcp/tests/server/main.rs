//! Integration tests for the r2e-mcp server core: the `McpServer` plugin,
//! `#[mcp_routes]` service registration and dispatch, generated schemas,
//! decorators (guards/interceptors), and serve lifecycle.

#[path = "../support/mod.rs"]
mod support;

mod fixtures;

mod dispatch;
mod interceptors;
mod lifecycle;
mod plugin;
mod prompts;
mod registry;
mod resources;
mod schema;
