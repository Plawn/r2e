use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::http::{Body, Method, Request, Response, StatusCode};
use crate::tunnel::TcpTunnel;

// ── CONNECT handler types ───────────────────────────────────────────────

/// A boxed async callback invoked for each CONNECT tunnel.
pub type ConnectHandlerFn<S> = Arc<
    dyn Fn(S, TcpTunnel) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

// ── ConnectLayer ────────────────────────────────────────────────────────

/// Tower layer that intercepts HTTP CONNECT requests before the Axum router.
///
/// Axum's `Router` does not support `Method::CONNECT`. This layer sits in
/// front and dispatches CONNECT to the registered handler while passing all
/// other requests through to the inner service.
#[derive(Clone)]
pub struct ConnectLayer<S: Clone> {
    handler: ConnectHandlerFn<S>,
    state: S,
}

impl<S: Clone> ConnectLayer<S> {
    pub fn new(handler: ConnectHandlerFn<S>, state: S) -> Self {
        Self { handler, state }
    }
}

impl<S: Clone, Svc> tower::Layer<Svc> for ConnectLayer<S> {
    type Service = ConnectService<S, Svc>;

    fn layer(&self, inner: Svc) -> Self::Service {
        ConnectService {
            inner,
            handler: self.handler.clone(),
            state: self.state.clone(),
        }
    }
}

/// The Tower service wrapping CONNECT interception.
#[derive(Clone)]
pub struct ConnectService<S, Svc> {
    inner: Svc,
    handler: ConnectHandlerFn<S>,
    state: S,
}

impl<S, Svc> tower::Service<Request> for ConnectService<S, Svc>
where
    S: Clone + Send + Sync + 'static,
    Svc: tower::Service<Request, Response = Response> + Clone + Send + 'static,
    Svc::Future: Send,
    Svc::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = Response;
    type Error = Svc::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Svc::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        if req.method() == Method::CONNECT {
            let handler = self.handler.clone();
            let state = self.state.clone();

            Box::pin(async move {
                let authority = req.uri().authority().cloned();
                let on_upgrade = req
                    .extensions_mut()
                    .remove::<crate::http::upgrade::OnUpgrade>();

                let (host, port) = match authority {
                    Some(auth) => (
                        auth.host().to_string(),
                        auth.port_u16().unwrap_or(443),
                    ),
                    None => {
                        return Ok(Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .body(Body::from("CONNECT: missing authority"))
                            .unwrap());
                    }
                };

                let on_upgrade = match on_upgrade {
                    Some(u) => u,
                    None => {
                        return Ok(Response::builder()
                            .status(StatusCode::BAD_GATEWAY)
                            .body(Body::from("CONNECT: upgrade not available"))
                            .unwrap());
                    }
                };

                tokio::spawn(async move {
                    match on_upgrade.await {
                        Ok(upgraded) => {
                            let tunnel = TcpTunnel::new(upgraded, host, port);
                            handler(state, tunnel).await;
                        }
                        Err(e) => {
                            tracing::error!(
                                host = host,
                                port = port,
                                "CONNECT upgrade failed: {e}"
                            );
                        }
                    }
                });

                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::empty())
                    .unwrap())
            })
        } else {
            let fut = self.inner.call(req);
            Box::pin(fut)
        }
    }
}

// ── ForwardProxyLayer ──────────────────────────────────────────────────

/// A boxed async callback invoked for each forward-proxy request (absolute URI).
pub type ForwardHandlerFn =
    Arc<dyn Fn(Request) -> Pin<Box<dyn Future<Output = Response> + Send>> + Send + Sync>;

/// Tower layer that intercepts forward-proxy requests (absolute URIs).
///
/// HTTP forward proxying uses `GET http://example.com/path HTTP/1.1`.
/// Axum routes by relative path, so these fall through. This layer detects
/// the absolute URI and dispatches to the registered handler.
#[derive(Clone)]
pub struct ForwardProxyLayer {
    handler: ForwardHandlerFn,
}

impl ForwardProxyLayer {
    pub fn new(handler: ForwardHandlerFn) -> Self {
        Self { handler }
    }
}

impl<Svc> tower::Layer<Svc> for ForwardProxyLayer {
    type Service = ForwardProxyService<Svc>;

    fn layer(&self, inner: Svc) -> Self::Service {
        ForwardProxyService {
            inner,
            handler: self.handler.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ForwardProxyService<Svc> {
    inner: Svc,
    handler: ForwardHandlerFn,
}

impl<Svc> tower::Service<Request> for ForwardProxyService<Svc>
where
    Svc: tower::Service<Request, Response = Response> + Clone + Send + 'static,
    Svc::Future: Send,
    Svc::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Response = Response;
    type Error = Svc::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Response, Svc::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        if is_forward_proxy_request(&req) {
            let handler = self.handler.clone();
            Box::pin(async move { Ok(handler(req).await) })
        } else {
            let fut = self.inner.call(req);
            Box::pin(fut)
        }
    }
}

fn is_forward_proxy_request(req: &Request) -> bool {
    req.uri().scheme().is_some() && req.method() != Method::CONNECT
}
