//! End-to-end tests booting the demo app against its shipped controllers.
//!
//! `ordered` relies on `#[r2e::test]` order/group barriers, which are scoped
//! to this binary; its tagged tests still serialize correctly among the
//! untagged tests of the sibling modules.

mod app;
mod boot_failure;
mod config;
mod order_module;
mod ordered;
mod proxy;
mod upload;
mod user_controller;
