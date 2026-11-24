//! Test utilities for R2E applications: boot the real app in process and drive
//! it with typed requests.
//!
//! Not part of the `r2e` facade: add `r2e-test` to `[dev-dependencies]`.
//!
//! # Main types
//!
//! - [`TestApp`] — boots an `App` exactly as production does
//!   ([`TestApp::boot`], [`TestApp::boot_with`] to override beans or config),
//!   forcing the `test` profile so `application-test.yaml` overlays the base
//!   config. Requests are built with `app.get(..)` / `app.post(..)` →
//!   [`TestRequest`] → [`TestResponse`] with assertion helpers.
//! - [`TestJwt`] / [`TokenBuilder`] — mint tokens accepted by the auto-pinned
//!   test validator; `.as_user(sub, roles)` on a request uses it.
//! - [`TestSession`] — cookie-carrying multi-request sessions.
//! - [`TestServer`] — a real listening server for clients that need a socket.
//! - SSE helpers ([`ParsedSseEvent`], [`FiniteStream`]); `WsTestClient` with
//!   feature `ws`, `GrpcTestChannel` with feature `grpc`.
//! - [`suite`] — runtime support for `#[r2e::test_suite]` (shared setup across
//!   cases).
//!
//! ```rust,ignore
//! use r2e::prelude::*;
//! use r2e_test::TestApp;
//!
//! #[r2e::test(app = my_app::MyApp)]
//! async fn lists_users(app: TestApp, #[inject] users: UserService) {
//!     app.get("/users").as_user("alice", &["user"]).send().await.assert_ok();
//!     assert_eq!(users.count().await, 2); // same bean instance the app used
//! }
//! ```
//!
//! # Guides
//!
//! [`docs/book/src/testing`](https://github.com/plawn/r2e/tree/master/docs/book/src/testing)
//! and the crate README.

#[macro_use]
mod app;
mod boot;
#[cfg(feature = "grpc")]
mod grpc;
mod jwt;
mod multipart;
mod resource;
pub mod ordering;
mod server;
mod session;
mod shared_env;
mod sse;
pub mod suite;
#[cfg(feature = "ws")]
mod ws;

pub use app::{
    json_contains, resolve_path, tokenize_path, PathToken, SameSite, SetCookie, TestApp,
    TestRequest, TestResponse, TENANT_CLAIM, TENANT_HEADER,
};
#[cfg(feature = "grpc")]
pub use grpc::GrpcTestChannel;
pub use jwt::{TestJwt, TokenBuilder};
pub use resource::{TestResource, TestResources};
pub use server::TestServer;
pub use session::{SessionRequest, TestSession};
pub use shared_env::{shared_env_runtime, SharedEnv, SharedEnvError, SharedEnvFuture};
pub use sse::{FiniteStream, ParsedSseEvent};
#[cfg(feature = "ws")]
pub use ws::{WsTestClient, WsTestError};
