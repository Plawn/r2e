//! Controllers: the per-request façade over an application-scoped core,
//! the core-only path (`#[anonymous]`, consumers, scheduled), injection
//! scopes, and catch-all / proxy routing.

#[path = "../support/mod.rs"]
mod support;

mod anonymous;
mod config;
mod core_path;
mod facade;
mod fixtures;
mod proxy_routes;
mod scope;
mod tuple;
