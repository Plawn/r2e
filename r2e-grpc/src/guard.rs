//! Bridge between gRPC method dispatch and R2E's shared guard machinery.
//!
//! gRPC reuses [`Guard<I>`](r2e_core::Guard) / [`GuardContext`] directly —
//! the same `#[roles]`, `#[all_roles]`, `#[guard]` specs (and every user
//! `#[derive(DecoratorBean)]` guard) work on `#[grpc_routes]` methods with
//! zero new impls. The generated tonic trait impl builds the context from
//! the incoming [`tonic::Request`] through [`guard_context`]; a denial is
//! projected onto a [`tonic::Status`] by kind through
//! [`rejection_to_status`](crate::rejection_to_status).
//!
//! What a gRPC guard sees in its [`GuardContext`]:
//!
//! | field | gRPC value |
//! |---|---|
//! | `method` | `POST` (every unary/streaming gRPC call is an HTTP/2 POST) |
//! | `headers` | the request metadata — `MetadataMap` is a `HeaderMap` view, so `headers.get("x-tenant-id")` reads a metadata key |
//! | `uri` | `/` — gRPC routing is by service/method, exposed as `controller_name`/`method_name` |
//! | `extensions` | the request extensions (tonic interceptors / tower layers can deposit values there) |
//! | `peer_addr` | [`tonic::Request::remote_addr`] |
//! | `path_params` | empty |
//! | `identity` | the method's `#[inject(identity)]` parameter, if any |

use r2e_core::http::{Method, Uri};
use r2e_core::{GuardContext, Identity, PathParams};

static GRPC_METHOD: Method = Method::POST;

fn default_uri() -> &'static Uri {
    static URI: std::sync::LazyLock<Uri> = std::sync::LazyLock::new(|| Uri::from_static("/"));
    &URI
}

/// Build a [`GuardContext`] for one gRPC call from the incoming request —
/// the form used by the generated `#[grpc_routes]` dispatch (see the module
/// docs for the field mapping).
pub fn guard_context<'a, T, I: Identity>(
    request: &'a tonic::Request<T>,
    method_name: &'static str,
    controller_name: &'static str,
    identity: Option<&'a I>,
) -> GuardContext<'a, I> {
    GuardContext {
        method_name,
        controller_name,
        method: &GRPC_METHOD,
        headers: request.metadata().as_ref(),
        uri: default_uri(),
        extensions: request.extensions(),
        peer_addr: request.remote_addr(),
        path_params: PathParams::EMPTY,
        identity,
    }
}
