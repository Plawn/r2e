//! HTTP surface: verbs, streaming (SSE/WS), rate limiting, OpenAPI mapping,
//! error envelopes.

mod error_envelope;
mod http_verbs;
mod openapi_unmapped_response;
mod rate_limit;
mod sse;
mod sse_bridge;
mod ws;
