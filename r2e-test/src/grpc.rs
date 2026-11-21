//! `app.grpc()` — call the app's gRPC services from a test, through the
//! generated tonic clients, with no socket.
//!
//! ```rust,ignore
//! let app = TestApp::boot::<MyApp>().await;
//! let mut greeter = GreeterClient::new(app.grpc().as_user("alice", &["admin"]));
//! let reply = greeter.say_hello(HelloRequest { name: "Bob".into() }).await?;
//! ```
//!
//! [`GrpcTestChannel`] stands where a `tonic::transport::Channel` would: it
//! hands each call to the service set the [`GrpcServer`](r2e_grpc::GrpcServer)
//! plugin serves — the registered services, plus reflection when enabled —
//! whichever transport the app picked. Guards, identity extraction and
//! interceptors run as they do over the wire; only the socket (and, on the
//! multiplexed transport, the content-type routing in front of the services)
//! is skipped.
//!
//! Behind `r2e-test`'s `grpc` feature.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::header::{HeaderName, HeaderValue, AUTHORIZATION};
use http::{HeaderMap, Request, Response};
use r2e_grpc::tonic::body::Body;
use r2e_grpc::tonic::service::Routes;
use r2e_grpc::GrpcMarker;
use tower::Service;

use crate::{TestApp, TestJwt};

impl TestApp {
    /// An in-process channel onto the app's gRPC services, for a generated
    /// tonic client — the gRPC counterpart of `app.get("/…")`:
    ///
    /// ```rust,ignore
    /// let mut greeter = GreeterClient::new(app.grpc().as_user("alice", &["admin"]));
    /// let reply = greeter.say_hello(HelloRequest { name: "Bob".into() }).await?;
    /// ```
    ///
    /// Anonymous unless an identity is chosen on the returned channel. Works
    /// with either `GrpcServer` transport; see [`GrpcTestChannel`].
    ///
    /// # Panics
    ///
    /// Panics if the app has no [`GrpcServer`](r2e_grpc::GrpcServer) plugin, or
    /// if no gRPC service was registered.
    pub fn grpc(&self) -> GrpcTestChannel {
        let (routes, names) = self.bean::<GrpcMarker>().served_routes().expect(
            "app.grpc(): no gRPC service is registered — call \
             `register_grpc_service::<S>()` (or list it in a module's \
             `grpc_services(..)`) in the app's blueprint",
        );
        GrpcTestChannel {
            routes: routes.prepare(),
            names,
            headers: HeaderMap::new(),
            jwt: self.jwt.clone(),
        }
    }
}

/// An in-process gRPC channel, from [`TestApp::grpc`].
///
/// Pass it to a generated client's `new`. It is cheap to clone, and every
/// identity/metadata method returns a new channel, so one test can hold several
/// callers side by side:
///
/// ```rust,ignore
/// let grpc = app.grpc();
/// let mut anonymous = GreeterClient::new(grpc.clone());
/// let mut admin = GreeterClient::new(grpc.as_user("alice", &["admin"]));
/// ```
///
/// Metadata set here is sent with every call of the client; metadata a call
/// sets itself (`tonic::Request::metadata_mut`) wins over it.
#[derive(Clone)]
#[must_use = "a GrpcTestChannel does nothing until a client is built on it"]
pub struct GrpcTestChannel {
    routes: Routes,
    names: Vec<&'static str>,
    headers: HeaderMap,
    jwt: Option<TestJwt>,
}

impl GrpcTestChannel {
    /// Call as `sub` with `roles`: an `authorization: Bearer …` metadata entry
    /// minted by the app's [`TestJwt`], exactly what
    /// [`TestRequest::as_user`](crate::TestRequest::as_user) sends over
    /// HTTP.
    ///
    /// # Panics
    ///
    /// Panics if the app has no `TestJwt` (`TestApp::boot` wires one;
    /// otherwise `.with_jwt(jwt)`).
    pub fn as_user(self, sub: &str, roles: &[&str]) -> Self {
        let token = self
            .jwt
            .as_ref()
            .expect(
                "app.grpc().as_user(..): this TestApp has no TestJwt — boot it with \
                 TestApp::boot, or attach one with .with_jwt(jwt)",
            )
            .token(sub, roles);
        self.bearer(&token)
    }

    /// Call with `authorization: Bearer <token>`.
    pub fn bearer(self, token: &str) -> Self {
        self.metadata(AUTHORIZATION, format!("Bearer {token}"))
    }

    /// Send an ASCII metadata entry with every call, replacing an earlier value
    /// of the same key.
    ///
    /// # Panics
    ///
    /// Panics if `key` or `value` is not a valid ASCII metadata key/value.
    pub fn metadata<K>(mut self, key: K, value: impl AsRef<str>) -> Self
    where
        K: TryInto<HeaderName>,
        K::Error: std::fmt::Debug,
    {
        let key = key.try_into().expect("invalid gRPC metadata key");
        let value = HeaderValue::from_str(value.as_ref()).expect("invalid gRPC metadata value");
        self.headers.insert(key, value);
        self
    }

    /// The full names of the services this channel reaches, in registration
    /// order (`greeter.Greeter`, …, then the reflection services when enabled).
    pub fn service_names(&self) -> &[&'static str] {
        &self.names
    }
}

impl std::fmt::Debug for GrpcTestChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcTestChannel")
            .field("services", &self.names)
            .finish_non_exhaustive()
    }
}

impl Service<Request<Body>> for GrpcTestChannel {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Service::<Request<Body>>::poll_ready(&mut self.routes, cx)
    }

    fn call(&mut self, mut request: Request<Body>) -> Self::Future {
        for (key, value) in &self.headers {
            if !request.headers().contains_key(key) {
                request.headers_mut().insert(key.clone(), value.clone());
            }
        }
        Box::pin(self.routes.call(request))
    }
}
