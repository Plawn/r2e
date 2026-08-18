//! Guards and interceptors: the `DecoratorSpec` contract, decorators built as
//! beans, and their end-to-end behavior around routes.

#[path = "../support/mod.rs"]
mod support;

mod bean;
mod config_keys;
mod controller_level;
mod e2e;
mod guards;
mod interceptors;
mod spec;
