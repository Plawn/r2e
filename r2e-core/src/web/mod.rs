//! Request-scoped HTTP surface: extractors, params, streaming, managed resources.

pub mod extract;
pub mod managed;
#[cfg(feature = "multipart")]
pub mod multipart;
pub mod pagination;
pub mod params;
pub mod request_head;
pub mod sse;
pub mod validation;
#[cfg(feature = "ws")]
pub mod ws;
