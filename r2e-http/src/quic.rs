// FACADE EXCEPTION: r2e-http sits below r2e-core in the dependency graph
// (r2e-core depends on r2e-http), so this file cannot use r2e_core::rt.
// tokio::spawn is called directly here.  The quinn/h3 libraries are also
// tokio-bound, so this is a permanent documented exception.
use bytes::{Buf, Bytes};
use std::net::SocketAddr;
use std::sync::Arc;

pub use quinn;

// ── Error ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum QuicError {
    Io(std::io::Error),
    Connection(quinn::ConnectionError),
    H3Connection(h3::error::ConnectionError),
    H3Stream(h3::error::StreamError),
    Tls(String),
}

impl std::fmt::Display for QuicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "QUIC I/O: {e}"),
            Self::Connection(e) => write!(f, "QUIC connection: {e}"),
            Self::H3Connection(e) => write!(f, "HTTP/3 connection: {e}"),
            Self::H3Stream(e) => write!(f, "HTTP/3 stream: {e}"),
            Self::Tls(e) => write!(f, "TLS: {e}"),
        }
    }
}

impl std::error::Error for QuicError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Connection(e) => Some(e),
            Self::H3Connection(e) => Some(e),
            Self::H3Stream(e) => Some(e),
            Self::Tls(_) => None,
        }
    }
}

impl From<std::io::Error> for QuicError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<quinn::ConnectionError> for QuicError {
    fn from(e: quinn::ConnectionError) -> Self {
        Self::Connection(e)
    }
}

impl From<h3::error::ConnectionError> for QuicError {
    fn from(e: h3::error::ConnectionError) -> Self {
        Self::H3Connection(e)
    }
}

impl From<h3::error::StreamError> for QuicError {
    fn from(e: h3::error::StreamError) -> Self {
        Self::H3Stream(e)
    }
}

// ── TLS configuration ──────────────────────────────────────────────────────

/// Build a [`quinn::ServerConfig`] from PEM-encoded certificate chain and private key.
///
/// Sets ALPN to `h3` for HTTP/3. Use [`build_server_config_with_alpn`] for
/// custom protocols.
pub fn build_server_config(
    cert_pem: &[u8],
    key_pem: &[u8],
) -> Result<quinn::ServerConfig, QuicError> {
    build_server_config_with_alpn(cert_pem, key_pem, vec![b"h3".to_vec()])
}

/// Build a [`quinn::ServerConfig`] with custom ALPN protocols.
pub fn build_server_config_with_alpn(
    cert_pem: &[u8],
    key_pem: &[u8],
    alpn_protocols: Vec<Vec<u8>>,
) -> Result<quinn::ServerConfig, QuicError> {
    let certs: Vec<_> = rustls_pemfile::certs(&mut &*cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| QuicError::Tls(format!("invalid certificate: {e}")))?;

    let key = rustls_pemfile::private_key(&mut &*key_pem)
        .map_err(|e| QuicError::Tls(format!("invalid private key: {e}")))?
        .ok_or_else(|| QuicError::Tls("no private key found in PEM".into()))?;

    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));

    let mut tls_config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| QuicError::Tls(e.to_string()))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| QuicError::Tls(e.to_string()))?;

    tls_config.alpn_protocols = alpn_protocols;

    let quic_config = quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
        .map_err(|e| QuicError::Tls(e.to_string()))?;

    Ok(quinn::ServerConfig::with_crypto(Arc::new(quic_config)))
}

/// Build a [`quinn::ServerConfig`] from PEM files on disk.
pub fn build_server_config_from_files(
    cert_path: &str,
    key_path: &str,
) -> Result<quinn::ServerConfig, QuicError> {
    let cert_pem = std::fs::read(cert_path)?;
    let key_pem = std::fs::read(key_path)?;
    build_server_config(&cert_pem, &key_pem)
}

// ── Streaming request body ────────────────────────────────────────────────

/// Channel-backed body that streams request data from an h3 stream into
/// an axum-compatible [`http_body::Body`].
struct ChannelBody {
    rx: tokio::sync::mpsc::Receiver<Bytes>,
}

impl http_body::Body for ChannelBody {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Bytes>, Self::Error>>> {
        match self.rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(data)) => {
                std::task::Poll::Ready(Some(Ok(http_body::Frame::data(data))))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

// ── HTTP/3 server ──────────────────────────────────────────────────────────

/// Default maximum request body size (2 MiB, matching axum's default).
pub const DEFAULT_MAX_BODY_SIZE: usize = 2 * 1024 * 1024;

/// Backpressure capacity for the channel between the h3 body reader and
/// the axum router. 4 chunks of in-flight data balances throughput with
/// memory usage for typical request bodies.
const BODY_CHANNEL_CAPACITY: usize = 4;

/// Run an HTTP/3 server that bridges requests into the given axum [`Router`](crate::Router).
///
/// Binds a QUIC endpoint on `bind_addr` using `server_config`, then accepts
/// HTTP/3 connections and forwards each request through the router — the same
/// controller pipeline as TCP/HTTP.
///
/// Request bodies are streamed through a channel rather than fully buffered,
/// but a running total is enforced against [`DEFAULT_MAX_BODY_SIZE`].
///
/// Returns when `shutdown` resolves or the endpoint is closed.
/// Closes and drains the endpoint before returning.
pub async fn serve_h3(
    router: crate::Router,
    bind_addr: SocketAddr,
    server_config: quinn::ServerConfig,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), QuicError> {
    let endpoint = quinn::Endpoint::server(server_config, bind_addr)?;
    let ep = endpoint.clone();
    serve_h3_with_endpoint(router, endpoint, shutdown).await?;
    ep.close(0u32.into(), b"shutdown");
    ep.wait_idle().await;
    Ok(())
}

/// Like [`serve_h3`] but with a pre-bound [`quinn::Endpoint`].
///
/// Useful when you need the server's actual bound address before starting
/// (e.g., in tests using port 0), or for dev-reload where the endpoint is
/// cached across cycles.
///
/// **Note:** This function does NOT close the endpoint on shutdown — the
/// caller manages endpoint lifecycle. Use [`serve_h3`] for full lifecycle
/// management.
pub async fn serve_h3_with_endpoint(
    router: crate::Router,
    endpoint: quinn::Endpoint,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> Result<(), QuicError> {
    let addr = endpoint.local_addr().map_err(QuicError::Io)?;
    tracing::info!(addr = %addr, "QUIC/HTTP3 endpoint listening");

    tokio::pin!(shutdown);

    let mut tasks = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let router = router.clone();
                tasks.spawn(async move {
                    if let Err(e) = handle_h3_connection(incoming, router).await {
                        tracing::debug!(error = %e, "HTTP/3 connection ended");
                    }
                });
            }
            _ = &mut shutdown => {
                tracing::info!("QUIC/HTTP3 endpoint shutting down");
                break;
            }
        }
    }

    tasks.shutdown().await;
    Ok(())
}

async fn handle_h3_connection(
    incoming: quinn::Incoming,
    router: crate::Router,
) -> Result<(), QuicError> {
    let connection = incoming.await?;
    let remote_addr = connection.remote_address();

    let mut h3_conn = h3::server::Connection::new(h3_quinn::Connection::new(connection))
        .await
        .map_err(QuicError::H3Connection)?;

    loop {
        match h3_conn.accept().await {
            Ok(Some(resolver)) => {
                let router = router.clone();
                tokio::spawn(async move {
                    match resolver.resolve_request().await {
                        Ok((req, stream)) => {
                            if let Err(e) =
                                handle_h3_request(req, stream, router, remote_addr).await
                            {
                                tracing::debug!(error = %e, "HTTP/3 request error");
                            }
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "HTTP/3 request resolve error");
                        }
                    }
                });
            }
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = %e, "HTTP/3 accept error");
                break;
            }
        }
    }

    Ok(())
}

async fn handle_h3_request<S>(
    req: http::Request<()>,
    mut stream: h3::server::RequestStream<S, Bytes>,
    router: crate::Router,
    remote_addr: SocketAddr,
) -> Result<(), QuicError>
where
    S: h3::quic::SendStream<Bytes> + h3::quic::RecvStream,
{
    let (parts, ()) = req.into_parts();

    // Fast-reject if Content-Length is present and already exceeds the limit.
    if let Some(cl) = parts.headers.get(http::header::CONTENT_LENGTH) {
        if let Some(len) = cl.to_str().ok().and_then(|s| s.parse::<usize>().ok()) {
            if len > DEFAULT_MAX_BODY_SIZE {
                let resp = http::Response::builder()
                    .status(http::StatusCode::PAYLOAD_TOO_LARGE)
                    .body(())
                    .unwrap();
                let _ = stream.send_response(resp).await;
                let _ = stream.finish().await;
                return Ok(());
            }
        }
    }

    // Create a channel-backed streaming body. The body reader feeds chunks
    // into `tx` while the router pulls them through `ChannelBody`.
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(BODY_CHANNEL_CAPACITY);
    let body = crate::Body::new(ChannelBody { rx });

    let mut request = http::Request::from_parts(parts, body);
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(remote_addr));

    use tower::ServiceExt as _;
    let router_future = router.oneshot(request);

    // Stream the request body concurrently with router processing.
    // `stream` is moved into the reader and returned so we can send
    // the response afterward.
    let body_reader = async {
        let mut stream = stream;
        let mut total = 0usize;
        loop {
            match stream.recv_data().await {
                Ok(Some(chunk)) => {
                    let data = Bytes::copy_from_slice(chunk.chunk());
                    total += data.len();
                    if total > DEFAULT_MAX_BODY_SIZE {
                        drop(tx);
                        return (stream, true);
                    }
                    if tx.send(data).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!(error = %e, "error reading h3 request body");
                    break;
                }
            }
        }
        drop(tx);
        (stream, false)
    };

    let ((mut stream, too_large), response) = tokio::join!(body_reader, router_future);

    if too_large {
        let resp = http::Response::builder()
            .status(http::StatusCode::PAYLOAD_TOO_LARGE)
            .body(())
            .unwrap();
        let _ = stream.send_response(resp).await;
        let _ = stream.finish().await;
        return Ok(());
    }

    let response: http::Response<crate::Body> = response.expect("axum router is infallible");

    // Send response headers
    let (resp_parts, resp_body) = response.into_parts();
    let h3_response = http::Response::from_parts(resp_parts, ());
    stream.send_response(h3_response).await?;

    // Stream response body
    use http_body_util::BodyExt as _;
    let mut body = resp_body;
    while let Some(frame_result) = body.frame().await {
        match frame_result {
            Ok(frame) => {
                if frame.is_data() {
                    if let Ok(data) = frame.into_data() {
                        stream.send_data(data).await?;
                    }
                } else if frame.is_trailers() {
                    if let Ok(trailers) = frame.into_trailers() {
                        stream.send_trailers(trailers).await?;
                    }
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "error reading response body");
                break;
            }
        }
    }

    stream.finish().await?;

    Ok(())
}

// ── Alt-Svc middleware ─────────────────────────────────────────────────────

/// Wrap a router with a middleware that adds an `Alt-Svc` header to every
/// TCP/HTTP response, advertising HTTP/3 availability on the given QUIC port.
///
/// Browsers that see `Alt-Svc: h3=":443"; ma=3600` will attempt an HTTP/3
/// upgrade on subsequent requests.
pub fn apply_alt_svc(router: crate::Router, port: u16, max_age: u32) -> crate::Router {
    let header_value = format!("h3=\":{port}\"; ma={max_age}");
    let header_value =
        http::HeaderValue::from_str(&header_value).expect("valid Alt-Svc header value");
    router.layer(axum::middleware::from_fn(
        move |req: crate::Request, next: axum::middleware::Next| {
            let hv = header_value.clone();
            async move {
                let mut response = next.run(req).await;
                response
                    .headers_mut()
                    .insert(http::header::HeaderName::from_static("alt-svc"), hv);
                response
            }
        },
    ))
}

// ── Raw QUIC endpoint ──────────────────────────────────────────────────────

/// A raw QUIC endpoint for custom (non-HTTP/3) protocols.
///
/// Wraps a [`quinn::Endpoint`] with convenience methods for accepting
/// connections and accessing bidirectional/unidirectional streams.
///
/// ```ignore
/// let config = build_server_config_with_alpn(&cert, &key, vec![b"my-proto".to_vec()])?;
/// let endpoint = QuicEndpoint::bind("0.0.0.0:4433".parse()?, config)?;
///
/// while let Some(conn) = endpoint.accept().await {
///     tokio::spawn(async move {
///         let conn = conn.await.unwrap();
///         let conn = QuicConnection::new(conn);
///         while let Ok((send, recv)) = conn.accept_bi().await {
///             // handle stream
///         }
///     });
/// }
/// ```
pub struct QuicEndpoint {
    inner: quinn::Endpoint,
}

impl QuicEndpoint {
    /// Bind a new QUIC endpoint to `addr`.
    pub fn bind(addr: SocketAddr, server_config: quinn::ServerConfig) -> Result<Self, QuicError> {
        let endpoint = quinn::Endpoint::server(server_config, addr)?;
        Ok(Self { inner: endpoint })
    }

    /// Accept the next incoming connection (handshake not yet complete).
    pub async fn accept(&self) -> Option<quinn::Incoming> {
        self.inner.accept().await
    }

    /// Local address this endpoint is bound to.
    pub fn local_addr(&self) -> Result<SocketAddr, QuicError> {
        self.inner.local_addr().map_err(QuicError::Io)
    }

    /// Gracefully close the endpoint.
    pub fn close(&self, reason: &[u8]) {
        self.inner.close(0u32.into(), reason);
    }

    /// Wait until all connections are closed.
    pub async fn wait_idle(&self) {
        self.inner.wait_idle().await;
    }

    /// Access the underlying [`quinn::Endpoint`] for advanced usage.
    pub fn inner(&self) -> &quinn::Endpoint {
        &self.inner
    }
}

/// A QUIC connection wrapper providing bidirectional and unidirectional
/// stream access.
pub struct QuicConnection {
    inner: quinn::Connection,
}

impl QuicConnection {
    /// Wrap an established quinn connection.
    pub fn new(conn: quinn::Connection) -> Self {
        Self { inner: conn }
    }

    /// Remote address of the peer.
    pub fn remote_address(&self) -> SocketAddr {
        self.inner.remote_address()
    }

    /// Accept the next bidirectional stream opened by the peer.
    pub async fn accept_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), QuicError> {
        self.inner.accept_bi().await.map_err(QuicError::Connection)
    }

    /// Open a new bidirectional stream.
    pub async fn open_bi(&self) -> Result<(quinn::SendStream, quinn::RecvStream), QuicError> {
        self.inner.open_bi().await.map_err(QuicError::Connection)
    }

    /// Accept the next unidirectional stream opened by the peer.
    pub async fn accept_uni(&self) -> Result<quinn::RecvStream, QuicError> {
        self.inner.accept_uni().await.map_err(QuicError::Connection)
    }

    /// Open a new unidirectional stream.
    pub async fn open_uni(&self) -> Result<quinn::SendStream, QuicError> {
        self.inner.open_uni().await.map_err(QuicError::Connection)
    }

    /// Gracefully close the connection.
    pub fn close(&self, code: u32, reason: &[u8]) {
        self.inner.close(quinn::VarInt::from_u32(code), reason);
    }

    /// Access the underlying [`quinn::Connection`].
    pub fn inner(&self) -> &quinn::Connection {
        &self.inner
    }
}
