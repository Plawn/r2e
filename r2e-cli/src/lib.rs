//! # r2e-cli
//!
//! Command-line tool for scaffolding and managing R2E projects.
//!
//! This crate provides the `r2e` binary with the following commands:
//!
//! | Command | Description |
//! |---------|-------------|
//! | `r2e new <name>` | Create a new R2E project with optional features |
//! | `r2e generate` | Generate controllers, services, CRUD, middleware, gRPC |
//! | `r2e add <ext>` | Add an R2E extension to Cargo.toml |
//! | `r2e dev` | Start development server with hot-reload |
//! | `r2e doctor` | Run project health diagnostics |
//! | `r2e routes` | List all declared routes from source |
//!
//! ## Architecture
//!
//! The CLI is organized into command modules under [`commands`]:
//!
//! - [`commands::new_project`] — project scaffolding (`r2e new`)
//! - [`commands::generate`] — code generation (`r2e generate`)
//! - [`commands::add`] — extension management (`r2e add`)
//! - [`commands::dev`] — development server (`r2e dev`)
//! - [`commands::doctor`] — project diagnostics (`r2e doctor`)
//! - [`commands::routes`] — route listing (`r2e routes`)
//! - [`commands::templates`] — shared template helpers and code templates

pub mod commands;
