//! Runtime proof for gRPC guards and identity (#1072 P3): `#[grpc_routes]`
//! methods run the shared `Guard<I>` family — `#[roles]`, `#[guard]`, impl-
//! level guards — over an `#[inject(identity)]` parameter extracted from the
//! `authorization` metadata by the `Arc<JwtClaimsValidator>` bean. A
//! rejection reaches the client as a `tonic::Status` projected by kind.

use std::future::Future;
use std::sync::{Arc, Mutex};

use r2e::prelude::*;
use r2e::r2e_grpc::GrpcService;
use r2e::r2e_rate_limit::{RateLimit, RateLimitRegistry};
use r2e::r2e_security::jwt::JwtClaimsValidator;
use r2e::r2e_security::AuthenticatedUser;
use r2e_test::TestJwt;

pub mod proto {
    r2e::r2e_grpc::include_protos!();
}

use proto::greeter::greeter_client::GreeterClient;
use proto::greeter::{HelloReply, HelloRequest};

// ── A header guard (no identity needed) ─────────────────────────────────

#[derive(DecoratorBean)]
pub struct ApiKey {
    expected: &'static str,
}

impl<I: Identity> Guard<I> for ApiKey {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Rejection>> + Send {
        let ok = ctx
            .headers
            .get("x-api-key")
            .is_some_and(|v| v.as_bytes() == self.expected.as_bytes());
        async move {
            if ok {
                Ok(())
            } else {
                Err(Rejection::forbidden("bad api key"))
            }
        }
    }
}

// ── Service ─────────────────────────────────────────────────────────────

#[controller]
pub struct GuardedGreeter {}

#[grpc_routes(proto::greeter::greeter_server::Greeter)]
#[guard(ApiKey::spec("k1"))]
impl GuardedGreeter {
    /// Impl-level guard only; identity optional (anonymous callers allowed).
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
        #[inject(identity)] user: Option<AuthenticatedUser>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let who = user.map(|u| u.sub().to_string()).unwrap_or_else(|| "anon".into());
        Ok(tonic::Response::new(HelloReply {
            message: format!("hi {} from {who}", request.get_ref().name),
        }))
    }

    /// Required identity + role check.
    #[roles("admin")]
    async fn say_hello_admin(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: format!("admin {} for {}", user.sub(), request.get_ref().name),
        }))
    }
}

// ── Test ────────────────────────────────────────────────────────────────

fn request(name: &str, api_key: Option<&str>, token: Option<&str>) -> tonic::Request<HelloRequest> {
    let mut req = tonic::Request::new(HelloRequest { name: name.into() });
    if let Some(key) = api_key {
        req.metadata_mut().insert("x-api-key", key.parse().unwrap());
    }
    if let Some(token) = token {
        req.metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    req
}

#[r2e::test]
async fn grpc_guards_and_identity_run_over_metadata() {
    let jwt = TestJwt::new();
    let builder = AppBuilder::new()
        .provide(Arc::new(jwt.claims_validator()) as Arc<JwtClaimsValidator>)
        .build_state()
        .await;

    // Registration: guard sets and the identity extractor are built here,
    // once, from the bean graph.
    let routes =
        GuardedGreeter::add_to_routes(tonic::service::Routes::default(), builder.bean_context());

    let listener = r2e::rt::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = r2e::rt::stream::wrappers::TcpListenerStream::new(listener);
    r2e::rt::spawn(async move {
        tonic::transport::Server::builder()
            .add_routes(routes)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let mut client = GreeterClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let admin = jwt.token("alice", &["admin"]);
    let viewer = jwt.token("bob", &["viewer"]);

    // Impl-level guard denies without the key — on every method.
    let err = client.say_hello(request("a", None, None)).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    assert_eq!(err.message(), "bad api key");

    // Optional identity: anonymous passes once the key is right.
    let resp = client
        .say_hello(request("a", Some("k1"), None))
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "hi a from anon");
    // …and a valid token is injected.
    let resp = client
        .say_hello(request("a", Some("k1"), Some(&viewer)))
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "hi a from bob");
    // …but a presented-and-invalid token is still rejected.
    let err = client
        .say_hello(request("a", Some("k1"), Some("garbage")))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    // Required identity: missing credential → Unauthenticated, before roles.
    let err = client
        .say_hello_admin(request("b", Some("k1"), None))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(err.message(), "Missing authorization metadata");

    // Wrong role → PermissionDenied (RolesGuard's RolesDenied by kind).
    let err = client
        .say_hello_admin(request("b", Some("k1"), Some(&viewer)))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::PermissionDenied);

    // Identity runs before guards: no key + no token is Unauthenticated,
    // not PermissionDenied.
    let err = client
        .say_hello_admin(request("b", None, None))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    // Everything right.
    let resp = client
        .say_hello_admin(request("b", Some("k1"), Some(&admin)))
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "admin alice for b");
}

// ── Guard-site scoping (controller `"*"` vs method context) ─────────────

/// Every `(controller_name, method_name)` a [`Recorder`] guard was checked
/// against, in call order.
static SEEN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Stateless recorder: logs the context it is checked against, always passes.
#[derive(DecoratorBean)]
pub struct Recorder {}

impl<I: Identity> Guard<I> for Recorder {
    fn check(
        &self,
        ctx: &GuardContext<'_, I>,
    ) -> impl Future<Output = Result<(), Rejection>> + Send {
        SEEN.lock()
            .unwrap()
            .push((ctx.controller_name.to_string(), ctx.method_name.to_string()));
        async { Ok(()) }
    }
}

#[controller]
pub struct ScopedGreeter {}

/// The same `RateLimit` spec at impl level (service-wide budget) and on one
/// method (its own budget): the two sites must key distinct buckets.
#[grpc_routes(proto::greeter::greeter_server::Greeter)]
#[guard(Recorder::spec())]
#[guard(RateLimit::per_user(1, 60))]
impl ScopedGreeter {
    #[guard(Recorder::spec())]
    #[guard(RateLimit::per_user(1, 60))]
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
        #[inject(identity)] user: AuthenticatedUser,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: format!("hello {} from {}", request.get_ref().name, user.sub()),
        }))
    }

    async fn say_hello_admin(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        Ok(tonic::Response::new(HelloReply {
            message: format!("admin {} for {}", user.sub(), request.get_ref().name),
        }))
    }
}

#[r2e::test]
async fn grpc_controller_and_method_guards_get_distinct_scopes() {
    let jwt = TestJwt::new();
    let builder = AppBuilder::new()
        .provide(Arc::new(jwt.claims_validator()) as Arc<JwtClaimsValidator>)
        .provide(RateLimitRegistry::default())
        .build_state()
        .await;
    let routes =
        ScopedGreeter::add_to_routes(tonic::service::Routes::default(), builder.bean_context());

    let listener = r2e::rt::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = r2e::rt::stream::wrappers::TcpListenerStream::new(listener);
    r2e::rt::spawn(async move {
        tonic::transport::Server::builder()
            .add_routes(routes)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    let mut client = GreeterClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let alice = jwt.token("alice", &[]);

    // The first valid call passes: the impl-level and the method-level
    // `RateLimit` sites each spend a token from their OWN bucket.
    let resp = client
        .say_hello(request("a", None, Some(&alice)))
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "hello a from alice");

    // The controller recorder saw the `"*"` context, the method recorder the
    // method's own — never the method name for a controller site. The
    // controller name is module-qualified, like HTTP.
    let seen = std::mem::take(&mut *SEEN.lock().unwrap());
    let qualified = concat!(module_path!(), "::ScopedGreeter");
    assert_eq!(
        seen,
        vec![
            (qualified.to_string(), "*".to_string()),
            (qualified.to_string(), "say_hello".to_string()),
        ]
    );

    // The impl-level budget is service-wide: one token for the whole
    // service, so the other method is now exhausted for alice too.
    let err = client
        .say_hello_admin(request("b", None, Some(&alice)))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::ResourceExhausted);

    // Another subject has its own buckets.
    let bob = jwt.token("bob", &[]);
    let resp = client
        .say_hello_admin(request("b", None, Some(&bob)))
        .await
        .unwrap();
    assert_eq!(resp.get_ref().message, "admin bob for b");
}
