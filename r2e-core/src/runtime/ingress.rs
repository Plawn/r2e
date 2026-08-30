//! Worker-affine ingress: `SO_REUSEPORT` socket helpers and the affinity
//! contract per transport.
//!
//! # The contract
//!
//! A worker-affine socket is one the kernel delivers to *this* worker only
//! (`SO_REUSEPORT` load-balancing across `N` sockets bound to one address).
//! R2E promises affinity **or an error** — never a silently shared socket:
//!
//! | Transport | Helper | Adopted with | On a platform without `SO_REUSEPORT` |
//! |---|---|---|---|
//! | TCP (HTTP) | [`reuseport_tcp`] | [`WorkerContext::adopt_tcp_listener`] | `server.workers` is refused at `run()` ([`UNSUPPORTED_PLATFORM_MSG`](super::sharded::UNSUPPORTED_PLATFORM_MSG)) |
//! | UDP | [`reuseport_udp`] | [`WorkerContext::adopt_udp`] | [`AffinityError::Unsupported`] |
//! | QUIC / HTTP3 | [`reuseport_udp`] → one `quinn::Endpoint` per worker over the adopted socket | [`WorkerContext::adopt_udp`] | [`AffinityError::Unsupported`] |
//!
//! [`reuseport_supported`] answers the question up front. There is no
//! "fall back to one shared socket" mode in the helpers: if an application
//! wants that, it binds a plain socket on the control plane and says so.
//!
//! # QUIC extension point
//!
//! R2E's own QUIC listener (`server.quic.*`) runs on the control plane. A
//! shard-local QUIC endpoint is a per-worker service: `reuseport_udp(addr)`
//! on the main thread (or in the factory), `worker.adopt_udp(sock)` inside
//! the factory, then `quinn::Endpoint::new_with_abstract_socket` /
//! `quinn::Endpoint::new(config, .., sock, runtime)` on that socket. The
//! endpoint, its connections and streams are then `!Send` state owned by the
//! worker exactly like any other `WorkerService`. R2E does not own the
//! protocol layer on top; see `examples/example-worker-udp` for the UDP shape
//! the QUIC variant extends.

use std::net::SocketAddr;

use super::worker::WorkerContext;

/// Why a worker-affine socket could not be created.
#[derive(Debug)]
pub enum AffinityError {
    /// `SO_REUSEPORT` is unavailable on this platform; the requested affinity
    /// cannot be honoured.
    Unsupported {
        /// `"tcp"` or `"udp"`.
        transport: &'static str,
    },
    /// The socket could not be created/bound.
    Io(std::io::Error),
}

impl std::fmt::Display for AffinityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { transport } => write!(
                f,
                "worker-affine {transport} ingress needs SO_REUSEPORT, which this platform \
                 does not support"
            ),
            Self::Io(e) => write!(f, "worker-affine socket: {e}"),
        }
    }
}

impl std::error::Error for AffinityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Unsupported { .. } => None,
        }
    }
}

impl From<std::io::Error> for AffinityError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// `true` when this platform supports `SO_REUSEPORT` (unix, excluding
/// solaris/illumos/cygwin) — i.e. when [`reuseport_tcp`] / [`reuseport_udp`]
/// can succeed and `server.workers` is accepted.
pub const fn reuseport_supported() -> bool {
    cfg!(all(
        unix,
        not(any(
            target_os = "solaris",
            target_os = "illumos",
            target_os = "cygwin"
        ))
    ))
}

#[cfg(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
))]
mod imp {
    use super::*;
    use socket2::{Domain, Protocol, Socket, Type};

    fn domain(addr: SocketAddr) -> Domain {
        if addr.is_ipv4() {
            Domain::IPV4
        } else {
            Domain::IPV6
        }
    }

    pub fn reuseport_tcp(addr: SocketAddr) -> Result<std::net::TcpListener, AffinityError> {
        let socket = Socket::new(domain(addr), Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
        socket.bind(&addr.into())?;
        socket.listen(1024)?;
        let listener: std::net::TcpListener = socket.into();
        // MANDATORY for rt::TcpListener::from_std.
        listener.set_nonblocking(true)?;
        Ok(listener)
    }

    pub fn reuseport_udp(addr: SocketAddr) -> Result<std::net::UdpSocket, AffinityError> {
        let socket = Socket::new(domain(addr), Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
        socket.bind(&addr.into())?;
        let sock: std::net::UdpSocket = socket.into();
        sock.set_nonblocking(true)?;
        Ok(sock)
    }
}

#[cfg(not(all(
    unix,
    not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
)))]
mod imp {
    use super::*;

    pub fn reuseport_tcp(_addr: SocketAddr) -> Result<std::net::TcpListener, AffinityError> {
        Err(AffinityError::Unsupported { transport: "tcp" })
    }

    pub fn reuseport_udp(_addr: SocketAddr) -> Result<std::net::UdpSocket, AffinityError> {
        Err(AffinityError::Unsupported { transport: "udp" })
    }
}

/// A non-blocking `SO_REUSEPORT` TCP listener bound to `addr` (backlog 1024),
/// ready for [`WorkerContext::adopt_tcp_listener`]. Bind one per worker to
/// the same address; the kernel spreads connections across them.
///
/// Port `0` works: read `local_addr()` from the first listener and bind the
/// others to it.
pub fn reuseport_tcp(addr: SocketAddr) -> Result<std::net::TcpListener, AffinityError> {
    imp::reuseport_tcp(addr)
}

/// A non-blocking `SO_REUSEPORT` UDP socket bound to `addr`, ready for
/// [`WorkerContext::adopt_udp`]. Bind one per worker to the same address;
/// the kernel spreads datagrams (by 4-tuple) across them.
pub fn reuseport_udp(addr: SocketAddr) -> Result<std::net::UdpSocket, AffinityError> {
    imp::reuseport_udp(addr)
}

impl WorkerContext {
    /// Register a std UDP socket with this worker's runtime. Must run on the
    /// worker thread (any `WorkerContext` method does); the socket must be
    /// non-blocking ([`reuseport_udp`] returns it that way).
    pub fn adopt_udp(&self, sock: std::net::UdpSocket) -> std::io::Result<crate::rt::UdpSocket> {
        self.assert_on_worker("adopt_udp");
        crate::rt::UdpSocket::from_std(sock)
    }

    /// Register a std TCP listener with this worker's runtime. Same rules as
    /// [`adopt_udp`](Self::adopt_udp).
    pub fn adopt_tcp_listener(
        &self,
        listener: std::net::TcpListener,
    ) -> std::io::Result<crate::rt::TcpListener> {
        self.assert_on_worker("adopt_tcp_listener");
        crate::rt::TcpListener::from_std(listener)
    }

    fn assert_on_worker(&self, what: &str) {
        assert_eq!(
            std::thread::current().id(),
            self.thread_id(),
            "WorkerContext::{what} called off worker {} thread",
            self.id()
        );
    }
}
