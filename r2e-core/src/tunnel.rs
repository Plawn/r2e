use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use hyper_util::rt::TokioIo;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pin_project! {
    /// Raw bidirectional TCP stream after an HTTP CONNECT upgrade.
    ///
    /// Analogous to [`WsStream`](crate::ws::WsStream) but without WebSocket
    /// framing — this is a plain byte pipe.
    pub struct TcpTunnel {
        #[pin]
        inner: TokioIo<crate::http::upgrade::Upgraded>,
        host: String,
        port: u16,
    }
}

impl TcpTunnel {
    pub(crate) fn new(upgraded: crate::http::upgrade::Upgraded, host: String, port: u16) -> Self {
        Self {
            inner: TokioIo::new(upgraded),
            host,
            port,
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn target(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Split into independent read and write halves.
    pub fn split(self) -> (TcpTunnelRead, TcpTunnelWrite) {
        let (r, w) = tokio::io::split(self);
        (TcpTunnelRead(r), TcpTunnelWrite(w))
    }

    /// Bidirectional copy between this tunnel and another async stream.
    ///
    /// Returns `(client_to_remote, remote_to_client)` byte counts.
    pub async fn pipe<T>(mut self, mut other: T) -> io::Result<(u64, u64)>
    where
        T: AsyncRead + AsyncWrite + Unpin,
    {
        tokio::io::copy_bidirectional(&mut self, &mut other).await
    }

    /// Unwrap into the raw hyper `Upgraded` connection (escape hatch).
    pub fn into_inner(self) -> crate::http::upgrade::Upgraded {
        self.inner.into_inner()
    }
}

impl AsyncRead for TcpTunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

impl AsyncWrite for TcpTunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.project().inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

/// Read half of a [`TcpTunnel`].
pub struct TcpTunnelRead(tokio::io::ReadHalf<TcpTunnel>);

impl AsyncRead for TcpTunnelRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

/// Write half of a [`TcpTunnel`].
pub struct TcpTunnelWrite(tokio::io::WriteHalf<TcpTunnel>);

impl AsyncWrite for TcpTunnelWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}
