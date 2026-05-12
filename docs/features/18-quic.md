# Feature 18 — QUIC / HTTP/3

## Objective

Provide HTTP/3 support (via QUIC) alongside the existing TCP/HTTP server, plus raw QUIC stream access for custom protocols.

## Feature Flag

```toml
# In r2e
r2e = { features = ["quic"] }
```

QUIC is intentionally **not** in the `full` feature set — it pulls heavy crypto dependencies (quinn, rustls, h3).

## Core Concepts

### HTTP/3 Bridge

When QUIC is configured, the server runs **two endpoints simultaneously**:
- **TCP** — standard HTTP/1.1 and HTTP/2 via axum
- **QUIC/UDP** — HTTP/3 via h3 + h3-quinn, bridged into the same axum `Router`

Both endpoints share the same controller pipeline: guards, interceptors, middleware, and error handling all work identically.

### Request Body Streaming

HTTP/3 request bodies are **streamed** through a channel rather than fully buffered. The h3 body reader and axum router run concurrently via `tokio::join!`, providing backpressure through an internal `mpsc` channel. A running total is enforced against `DEFAULT_MAX_BODY_SIZE` (2 MiB). If the client sends a `Content-Length` header exceeding the limit, the server rejects immediately with 413.

### Alt-Svc Header

When QUIC is configured, the builder automatically injects an `Alt-Svc` response header on all TCP responses:

```
Alt-Svc: h3=":4433"; ma=3600
```

This tells browsers that HTTP/3 is available. The port and max-age are configurable.

### Raw QUIC Streams

For custom (non-HTTP/3) protocols, `QuicEndpoint` and `QuicConnection` provide thin wrappers around quinn with bidirectional and unidirectional stream access.

## Configuration

QUIC is configured via `application.yaml` under the `server.quic` section:

```yaml
server:
  host: 0.0.0.0
  port: 3000
  quic:
    port: 4433                   # UDP port for QUIC/HTTP3
    cert: certs/server.crt       # PEM certificate chain path
    key: certs/server.key        # PEM private key path
    alt_svc_max_age: 3600        # Alt-Svc header max-age in seconds (default: 3600)
```

| Key | Type | Required | Default | Description |
|-----|------|----------|---------|-------------|
| `server.quic.port` | `u16` | Yes | — | UDP port for the QUIC endpoint |
| `server.quic.cert` | `String` | Yes | — | Path to PEM certificate chain |
| `server.quic.key` | `String` | Yes | — | Path to PEM private key |
| `server.quic.alt_svc_max_age` | `u32` | No | `3600` | Alt-Svc header max-age (seconds) |

If `server.quic.port` is set but `cert` or `key` is missing, the error is logged and QUIC is skipped (TCP-only).

## Usage

### Automatic (via config)

With YAML config in place, `serve_auto()` or `serve(addr)` automatically starts the QUIC endpoint:

```rust
AppBuilder::new()
    .load_config::<AppConfig>()
    .build_state::<Services, _, _>().await
    .register_controller::<MyController>()
    .serve_auto()
    .await?;
```

### Manual HTTP/3 Server

For programmatic control (e.g., tests):

```rust
use r2e::http::quic;

let config = quic::build_server_config(&cert_pem, &key_pem)?;
let router = r2e::http::Router::new().route("/ping", get(|| async { "pong" }));

quic::serve_h3(router, "0.0.0.0:4433".parse()?, config, shutdown_signal()).await?;
```

Or with a pre-bound endpoint (useful for tests to avoid port races):

```rust
let endpoint = quinn::Endpoint::server(config, addr)?;
let server_addr = endpoint.local_addr()?;

quic::serve_h3_with_endpoint(router, endpoint, shutdown_signal()).await?;
```

**Note:** `serve_h3_with_endpoint` does NOT close the endpoint — the caller manages lifecycle. `serve_h3` handles full lifecycle (close + drain).

### Raw QUIC Streams

```rust
use r2e::http::quic::{self, QuicEndpoint, QuicConnection, build_server_config_with_alpn};

let config = build_server_config_with_alpn(&cert, &key, vec![b"my-proto".to_vec()])?;
let endpoint = QuicEndpoint::bind("0.0.0.0:4433".parse()?, config)?;

while let Some(incoming) = endpoint.accept().await {
    tokio::spawn(async move {
        let conn = QuicConnection::new(incoming.await.unwrap());
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();

        let mut buf = vec![0u8; 1024];
        let n = recv.read(&mut buf).await.unwrap().unwrap();
        send.write_all(&buf[..n]).await.unwrap();
        send.finish().unwrap();
    });
}
```

## API Reference

### TLS Configuration

| Function | Description |
|----------|-------------|
| `build_server_config(cert_pem, key_pem)` | Build `quinn::ServerConfig` with ALPN `h3` |
| `build_server_config_with_alpn(cert_pem, key_pem, alpn)` | Custom ALPN protocols |
| `build_server_config_from_files(cert_path, key_path)` | Load PEM from disk |

### HTTP/3 Server

| Function | Description |
|----------|-------------|
| `serve_h3(router, addr, config, shutdown)` | Bind + serve + close lifecycle |
| `serve_h3_with_endpoint(router, endpoint, shutdown)` | Serve with pre-bound endpoint (no close) |
| `apply_alt_svc(router, port, max_age)` | Wrap router with Alt-Svc middleware |

### Raw QUIC

| Type | Description |
|------|-------------|
| `QuicEndpoint` | UDP endpoint wrapper (bind, accept, close) |
| `QuicConnection` | Connection wrapper (accept_bi, open_bi, accept_uni, open_uni) |
| `QuicError` | Error enum (Io, Connection, H3Connection, H3Stream, Tls) |

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `DEFAULT_MAX_BODY_SIZE` | 2 MiB | Maximum request body size for HTTP/3 |

### Re-exports

The `quinn` crate is re-exported as `r2e::http::quic::quinn` for advanced usage.

## Dev-Reload

When `dev-reload` is enabled, the QUIC endpoint is cached across hot-reload cycles:

- The UDP socket survives hot-patches without port conflicts
- The accept loop stops and restarts with the new router
- **TLS certificate changes require a full process restart** (the cached endpoint retains the original cert)

## Crate Architecture

```
r2e-http (feature "quic")
  └─ src/quic.rs
       ├─ QuicError
       ├─ TLS config builders
       ├─ ChannelBody (streaming request body)
       ├─ serve_h3 / serve_h3_with_endpoint
       ├─ handle_h3_connection / handle_h3_request
       ├─ apply_alt_svc (middleware)
       ├─ QuicEndpoint / QuicConnection
       └─ re-exports quinn

r2e-core (feature "quic")
  └─ builder.rs — QUIC config extraction, endpoint spawn, Alt-Svc wiring
  └─ dev.rs — QUIC endpoint caching (dev-reload + quic)
```

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| quinn | 0.11 | QUIC transport |
| h3 | 0.0.8 | HTTP/3 protocol |
| h3-quinn | 0.0.10 | h3 ↔ quinn adapter |
| rustls | 0.23 (ring) | TLS implementation |
| rustls-pemfile | 2 | PEM parsing |
