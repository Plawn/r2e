//! Attribute extraction utilities, organized by domain.
//!
//! - `route`: HTTP route attributes (#[get], #[post], #[roles], etc.)
//! - `consumer`: Event consumer attributes (#[consumer])
//! - `scheduled`: Scheduled task attributes (#[scheduled])
//! - `managed`: Managed resource attributes (#[managed])

pub mod async_exec;
pub mod consumer;
pub mod duration;
pub mod managed;
pub mod plugins;
pub mod route;
pub mod scheduled;

// Re-export all public items for backward compatibility
pub use async_exec::*;
pub use consumer::*;
pub use managed::*;
pub use plugins::{parse_decorators, parse_grpc_decorators, strip_known_attrs};
pub use route::*;
pub use scheduled::*;
